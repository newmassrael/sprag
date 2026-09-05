//! `sprag` — the session-management CLI for a running `sprag-term` daemon.
//!
//! ```text
//! sprag ls                 list every session
//! sprag list-clients [-t SESSION]  list attached clients and the session each views (tmux list-clients)
//! sprag new [name] [-a]    create a session with a shell (absent name -> the lowest free), print its
//!                          name; -a instead opens a WINDOW on a new session (the window creates it,
//!                          so nothing is printed) — the explicit "new", since a bare launch adopts
//! sprag ssh [user@]host [-p PORT] [-L FWD]... [--tmux[=NAME]] [-- cmd...]  create a session running
//!                          ssh to a remote host (a first-classed remote workspace); -L forwards a
//!                          local->remote port; --tmux attaches-or-creates a remote tmux session
//! sprag find NEEDLE [-t SESSION] [--pane PANE] [--regex]  print each matching line as
//!                          PANE:LINE: text. Literal + ASCII case-insensitive by default;
//!                          --regex reads NEEDLE as a case-SENSITIVE regular expression (use
//!                          (?i) to fold); --pane narrows the sweep to one pane
//! sprag wait-for-output --pane PANE NEEDLE [-t SESSION] [--regex]  BLOCK until that pane's retained
//!                          output matches, then print the matching lines like `find`. The same two
//!                          search languages, in the other tense: `find` asks "does it say this
//!                          now", this asks "tell me when it does". No timeout — wrap it in
//!                          `timeout` if you want one
//! sprag run [NAME] [-t SESSION] [--pane PANE]  list the commands the pane's project declares
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
//! sprag select-pane -t SESSION <PANE | -L|-R|-U|-D [--from PANE]>
//!                                         make a pane ACTIVE — by PANE, or by walking the
//!                                         arrangement left/right/up/down from the pane the
//!                                         session is on, or from the pane --from names (tmux
//!                                         select-pane). Session state: every
//!                                         attached client follows, and a pane verb given no
//!                                         target acts on it
//! sprag swap-pane -t SESSION [PANE] <WITH | -L|-R|-U|-D>
//!                                         EXCHANGE two panes' places — the same walk, moving the
//!                                         pane instead of the cursor. The leading PANE is the
//!                                         origin (default: the active pane), which is what
//!                                         select-pane spells --from
//!
//! sprag windows -t SESSION                list a session's windows (name, and which is current)
//! sprag new-window -t SESSION [-d] [name]  create + select a window, born with a shell; print its
//!                                          name. -d creates it WITHOUT selecting (tmux -d), so a
//!                                          caller can make a place to work without moving anyone
//! sprag select-window -t SESSION <NAME|-n|-p>  make NAME current, or step along the window ring
//! sprag move-window -t SESSION [NAME] <--first|--last|-n|-p|--before W|--after W>
//!                                          move a window's PLACE in the session's order
//! sprag rename-window -t SESSION [win] NEW rename a window (default: the current one) to NEW
//! sprag rename-session [-t SESSION] NEW   rename a session. A session NAME is the address every
//!                                         -t takes, so the daemon carries the session's parked
//!                                         clients and its attachments across with it
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
//! sprag split-window [-t SESSION] [-w WINDOW] [-c DIR] [-h|-v [-b] PANE] [-- command…]
//!                                         divide PANE right (-h) / below (-v), or append with
//!                                         neither; print the new pane's id (tmux split-window).
//!                                         -c is where it STARTS, -w is which window it stands in
//!                                         (default: the window the caller is standing in)
//! sprag rename-pane [-t SESSION] PANE <NAME | --clear>  give a pane a NAME, or take it away.
//!                                         A name is an ADDRESS an agent can hold where a pane
//!                                         NUMBER goes stale; unique across the daemon
//! sprag kill-pane [-t SESSION] PANE               close a pane (tmux kill-pane)
//! sprag resize-pane [-t SESSION] [PANE] -x COLS -y ROWS  resize a pane's PTY + emulator
//! sprag resize-pane [-t SESSION] [PANE] -L|-R|-U|-D [N]  move the boundary beside a pane
//! sprag send-keys [-t SESSION] PANE [-l] KEY…     send W3C key names (or, with -l, literal text)
//! sprag capture-pane [-t SESSION] PANE [-p] [--line-breaks screen|program]
//!                                                 print a pane's retained output to stdout
//! sprag agent [-t SESSION] [PANE]                 what the AI agent in each pane is doing
//! sprag report-agent STATE [--pane PANE] [--source S]  say what the agent in a pane is DOING
//!                          [--name AGENT] [--seq N]  (the pane defaults to $SPRAG_PANE)
//!                          [--asked PROMPT]          --asked is what ENDS a turn: see the verb
//!                          [--said ANSWER]           --said is what a convergence is decided on
//!                          [--transcript PATH]       and --transcript is what makes spend readable
//! sprag release-agent [-t SESSION] [--pane PANE]      hand the pane back to screen inference
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
//! sprag words [NAME]                       print the closed vocabularies a run's answer speaks —
//!                                          `status`, `outcome`, `verdict`, `refusal` — from the
//!                                          types this build compiled. NEEDS NO DAEMON and no
//!                                          source tree; naming one prints only that one
//! sprag disposition [OUTCOME]              print WHAT HAPPENS NEXT to a run that ended — one line
//!                                          per ending, its next step and what that means. NEEDS NO
//!                                          DAEMON; naming one ending prints only that one, and an
//!                                          ending nothing classifies is REFUSED, not answered
//! sprag waits [LOG]                        print HOW LONG each working tree had no run driving it
//!                                          — one line per stretch, plus a line per run the logs
//!                                          cannot measure and why. NEEDS NO DAEMON (it reads the
//!                                          run logs on disk); naming a LOG reads only that one
//! sprag folds [LOG]                        print HOW FULL each session was when it folded the
//!                                          prompts it was sent — one line per run that can be
//!                                          read, plus a line per run the logs cannot read and
//!                                          why. NEEDS NO DAEMON; naming a LOG reads only that one
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
//! ## A PANE argument is an ADDRESS
//!
//! Every `PANE` above is a pane's id or its NAME, told apart by
//! [`PaneAddress`] — the same rule the agent surface uses,
//! and the reason [`PaneName`](sprag_terminal::PaneName) refuses an all-digit name. Either
//! spelling reaches any WINDOW of the scoped session, and neither crosses a session.
//!
//! **Both halves of that were false before R312.** No verb accepted a name at all, and the six
//! spellings of that refusal (`pane id "x" must be a number` / `"x" is not a pane id` / `"x" is
//! neither a direction flag nor a pane id` / `"x" is neither a flag nor a pane id` / `"x" is
//! neither -t nor a pane id` / `--pane "x" is not a pane id (a number)`) are what a rule with no
//! home looks like. Worse, the verbs disagreed about whether a pane EXISTED: `zoom-pane`,
//! `rename-pane` and `swap-pane` reached a pane one window over — they are registry-wide mux
//! actions — while `capture-pane`, `agent` and `select-pane` refused the same pane at the same
//! instant, because they pre-flighted against the scoped session's CURRENT WINDOW.
//!
//! There is one resolver now ([`resolve_pane`]), it answers a [`PaneSite`] that carries the window
//! it found, and a `PaneSite` has no other constructor — so a verb cannot address a pane it has
//! not looked up. `every_verb_the_usage_says_takes_a_pane_reaches_one_a_window_over` derives its
//! list from the usage text above and fails for a verb that stops.
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

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_host::events::EventKind;
use sprag_host::hooks::{self, HookError, Target};
use sprag_host::keymap::{BoundAction, KeySpec, KeyTable};
use sprag_host::pane_address::{
    NamedPane, PaneAddress, PaneListing, ambiguous_pane_name, unknown_pane_name_with,
};
use sprag_host::shellword::shell_quote;
use sprag_host::vocabulary::{self, Verb};
use sprag_host::window::SizeRequest;
use sprag_host::wire::{
    ACTION_GRAMMAR_SLOT, AGENT_MANIFESTS_SLOT, BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION,
    DISPLAY_MESSAGE_ACTION, DOCTOR_WINDOW, ENDED_KEY, GRANT_PANE_ACTION, JOIN_PANE_ACTION,
    KEY_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, LAYOUT_SLOT, LineBreaks, MOVE_PANE_ACTION,
    MOVE_WINDOW_ACTION, MoveWindowAsk, NEEDLE_PARAM, NEW_SESSION_ACTION, NEW_WINDOW_ACTION,
    PANE_DRIVEN_KEY, PANE_PARAM, PANE_REVIVED_KEY, PANE_WAIT_OUTPUT_METHOD, PANES_SLOT,
    PASTE_ACTION, PATTERN_PARAM, PaneProcessesWire, PaneResourcesWire, RELEASE_AGENT_ACTION,
    RENAME_PANE_ACTION, RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION, REPORT_AGENT_ACTION,
    RESIZE_ACTION, RESIZE_PANE_ACTION, RESIZE_WINDOW_ACTION, ResizeAsk, ResizeHow, ResizeWindowAsk,
    SELECT_PANE_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT, SPAWN_ACTION, SPLIT_ACTION,
    SWAP_PANE_ACTION, SelectAsk, SelectHow, SelectWindowAsk, SwapAsk, SwapHow, TEXT_ACTION,
    TREE_SLOT, UNSIGNALLED_KEY, UNSIGNALLED_WHICH_KEY, UNSIGNALLED_WHY_KEY, WINDOWS_SLOT,
    WindowBirthAsk, WindowPin, ZOOM_PANE_ACTION, doctor_over, events_slot_since, find_slot_for,
    pane_processes_at, pane_resources_at, project_slot_for, regex_slot_for, session_activity_at,
    settled, unknown_action, unknown_slot,
};
use sprag_host::{ClientSize, PaneFind, SshTarget, mux_action_path, pane_input_path};
use sprag_rpc::{
    CallError, EVENTS_CHANGED_METHOD, EVENTS_SUBSCRIBE_METHOD, Flag, HOST_SOCKET, HostConn,
    HostEndpoint, INVALID_PARAMS, PublishedForm, RpcFault, SINCE_PARAM, socket_path,
};
use sprag_terminal::{
    Ceiling, Counted, Cpu, Diagnosis, Ended, LayoutSnapshot, OrderStep, PaneDir, PaneId, PlaceHow,
    SignalKey, Taken, Unraised, Verdict, Waiting, WindowPlace, arrangement,
};

/// A management command is talking to an already-running daemon, so the socket either accepts
/// at once or there is nothing to manage — no spawn-race retry to wait out.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a REQUEST verb waits for the daemon's answer before saying it is not coming.
///
/// Every verb but the two that park is a request-response against a LOCAL daemon answering out of
/// memory, so the honest scale here is microseconds — R340 measured the most expensive read on the
/// wire, a `/proc` walk over eight panes, at 5.2 ms. Five seconds is that with four orders of
/// magnitude of headroom, which is what keeps this from firing on a box under the 4x
/// oversubscription this project's own gates run at. It is a *this is not coming* threshold, not a
/// performance budget.
///
/// Longer than [`HOOK_DEADLINE`] on purpose: this is somebody waiting at their own terminal, and
/// that one runs inside an agent's critical path where a person is waiting for something else.
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

fn main() {
    if let Err(error) = run() {
        eprintln!("sprag: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(word) = args.next() else {
        print_usage();
        return Ok(());
    };
    // ONE place a word becomes a verb, shared with the keyboard and with the help this prints —
    // see [`sprag_host::vocabulary`]. A word that names nothing is the only unknown left: a verb
    // this vocabulary HAS and no shell dispatches is answered by `dispatch` in its own words,
    // because a user who typed `sprag switch-client` asked for something that exists.
    let Some(verb) = Verb::parse(&word) else {
        eprintln!("sprag: unknown command {word:?}");
        print_usage();
        std::process::exit(2);
    };
    dispatch(verb, args)
}

/// Carry out one verb of the vocabulary.
///
/// **EXHAUSTIVE over [`Verb`], and that is the whole mechanism this round added**: a verb added to
/// the table without an implementation here does not compile, and a verb implemented here without a
/// table entry cannot be reached — so the dispatch, the help
/// ([`vocabulary::usage`]) and the keyboard's list of forms are three
/// projections of one array instead of three lists that drift. Two of them had: `run` and `hook`
/// were dispatched and named in no usage text at all.
fn dispatch(verb: Verb, mut args: impl Iterator<Item = String>) -> io::Result<()> {
    match verb {
        Verb::Ls => ls(),
        Verb::ListClients => list_clients(args.collect()),
        Verb::New => new(args.collect()),
        Verb::Ssh => ssh(args.collect()),
        Verb::Find => find(args.collect()),
        Verb::WaitForOutput => wait_for_output(args.collect()),
        Verb::Run => run_project(args.collect()),
        Verb::Attach => attach(args.collect()),
        Verb::KillSession => kill_session(args.next()),
        Verb::KillServer => kill_server(args.collect()),
        Verb::Windows => windows(args.collect()),
        Verb::NewWindow => new_window(args.collect()),
        Verb::SelectWindow => select_window(args.collect()),
        Verb::MoveWindow => move_window(args.collect()),
        Verb::SelectPane => select_pane(args.collect()),
        Verb::RenameWindow => rename_window(args.collect()),
        Verb::RenameSession => rename_session(args.collect()),
        Verb::KillWindow => kill_window(args.collect()),
        Verb::ResizeWindow => resize_window(args.collect()),
        Verb::BreakPane => break_pane(args.collect()),
        Verb::JoinPane => join_pane(args.collect()),
        Verb::MovePane => move_pane(args.collect()),
        Verb::SwapPane => swap_pane(args.collect()),
        Verb::ZoomPane => zoom_pane(args.collect()),
        Verb::RenamePane => rename_pane(args.collect()),
        Verb::Panes => panes(args.collect()),
        Verb::Layout => layout(args.collect()),
        Verb::Processes => processes(args.collect()),
        Verb::Resources => resources(args.collect()),
        Verb::Grant => grant(args.collect()),
        Verb::Doctor => doctor(args.collect()),
        Verb::Words => words(args.collect()),
        Verb::Disposition => disposition(args.collect()),
        Verb::Waits => waits(args.collect()),
        Verb::Folds => folds(args.collect()),
        Verb::Daemons => daemons(args.collect()),
        Verb::ShowGrammar => show_grammar(args.collect()),
        Verb::Orchestrate => orchestrate(args.collect()),
        Verb::Runs => runs(args.collect()),
        Verb::MyRuns => my_runs(args.collect()),
        Verb::CancelRun => cancel_run(args.collect()),
        Verb::StandDown => stand_down(args.collect()),
        Verb::HoldRun => hold_run(args.collect(), true),
        Verb::ResumeRun => hold_run(args.collect(), false),
        Verb::Agent => agent(args.collect()),
        Verb::AnswerPane => answer_pane(args.collect()),
        Verb::DisplayMessage => display_message(args.collect()),
        Verb::ReportAgent => report_agent(args.collect()),
        Verb::ReleaseAgent => release_agent(args.collect()),
        Verb::InstallHooks => install_hooks(args.collect()),
        Verb::UninstallHooks => uninstall_hooks(args.collect()),
        Verb::ListHooks => list_hooks(args.collect()),
        Verb::Hook => hook(args.collect()),
        Verb::Events => events(args.collect()),
        Verb::SplitWindow => split_window(args.collect()),
        Verb::KillPane => kill_pane(args.collect()),
        Verb::StopJob => stop_job(args.collect()),
        Verb::ResizePane => resize_pane(args.collect()),
        Verb::SendKeys => send_keys(args.collect()),
        Verb::CapturePane => capture_pane(args.collect()),
        Verb::ListKeys => list_keys(args.collect()),
        Verb::BindKey => bind_key(args.collect()),
        Verb::UnbindKey => unbind_key(args.collect()),
        Verb::ShowOptions => show_options(args.collect()),
        Verb::SetOption => set_option(args.collect()),
        Verb::Version => {
            print_version();
            Ok(())
        }
        Verb::Help => {
            print_usage();
            Ok(())
        }
        // THE FIVE VERBS A SHELL CANNOT SAY, each acting on the client that pressed the key. They
        // are refused BY NAME with the line that would bind them, where until R323 they came back
        // `unknown command "switch-client"` — a sentence about a verb this product has had since
        // R314. An argument error, so it exits 1 like every other one; the unknown-word path above
        // keeps its 2.
        Verb::DetachClient
        | Verb::SendPrefix
        | Verb::SwitchClient
        | Verb::ChooseTree
        | Verb::ConfirmBefore => Err(bad_input(
            &verb
                .only_a_keystroke()
                // Unreachable: these five ARE the verbs with no shell form, which is the question
                // that method answers. A fallback rather than an `expect` because this renders an
                // error message, and a panic inside one is a worse outcome than a plainer sentence.
                .unwrap_or_else(|| format!("{:?} is not a command", verb.name())),
        )),
        // THE THREE ACTS THIS SHELL DOES NOT SPELL YET, which R335's join added to the vocabulary:
        // sprag reads a pane's last command, its hyperlinks and its inline images, and serves all
        // three to an AI agent and to no shell. They are answered by NAME for the five above's
        // reason — a person who typed one asked for something the product does — and the sentence
        // names the mouth that has it, so it is a place to go rather than a refusal.
        Verb::ReadLastCommand | Verb::PaneLinks | Verb::PaneImages => Err(bad_input(
            &verb
                .no_shell_form_yet()
                // Unreachable: these three ARE the verbs whose shell answer is `NotBuilt`, which is
                // the question that method answers. A fallback rather than an `expect` for the
                // reason above.
                .unwrap_or_else(|| format!("{:?} is not a command", verb.name())),
        )),
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
    let mut pane: Option<String> = None;
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
                pane = Some(
                    it.next()
                        .ok_or_else(|| bad("run: --pane needs a pane id or NAME".to_owned()))?,
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
    let site = resolve_optional_pane(&mut conn, session.as_deref(), pane.as_deref(), "run")?;
    let pane = match &site {
        Some(site) => site.id,
        None => *pane_ids(&mut conn, session.as_deref())?
            .first()
            .ok_or_else(|| bad("run: the window holds no pane".to_owned()))?,
    };

    let answer: Value = query_slot(&mut conn, scoped(mux_action_path(&project_slot_for(pane))))?;
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
    invoke_action(&mut conn, {
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

/// ⛔⛔⛔⛔⛔ `words [NAME]`: **WHAT THIS BUILD'S RUN ROWS SPEAK, ASKED OF THIS BUILD** — register
/// item 773, and the answer that replaces three `grep`s into a source tree.
///
/// # ⚠⚠⚠⚠⚠ Why a verb, when a document could hold the same four lists
///
/// Because it could not. Item 773's subject is the loop skill, which had grown three vocabulary
/// TABLES with *"ask the product, not this table"* written above them; every one of them aged. The
/// tables were replaced by `grep -n -A7 'const fn wire_str' crates/…` — better, and still a copy:
/// an address ages the moment a symbol moves, silently, and **it cannot be run from another
/// repository**, which is where that skill's own last section says the loop is driven from.
///
/// This needs **no daemon** ([`list_keys`]'s reason, one axis over: the words are compiled in, not
/// served) and **no source tree**, so the two ways the old answer failed are both gone.
///
/// # ⚠⚠⚠ An unknown NAME is a REFUSAL that names what there is
///
/// This workspace's rule 6: an unclassified case is not a pass. A caller who asked for `outcomes`
/// and got silence would read it as *this build has none*, so the refusal lists the four rather
/// than sending anybody back to a source tree to find out.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a name no vocabulary has, and for a second argument.
fn words(args: Vec<String>) -> io::Result<()> {
    let known = sprag_host::plugins::RUN_VOCABULARIES;
    if let Some(extra) = args.get(1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("words: unexpected argument {extra:?} (it takes [NAME], one at a time)"),
        ));
    }
    // ⚠ THE FILTER IS THE ONLY BRANCH. Printing every vocabulary and printing one are the same act
    // over a different set, so a `NAME` cannot come to be formatted differently from the whole —
    // which is what a second printing arm would eventually do.
    let wanted = args.first();
    let shown: Vec<_> = known
        .iter()
        .filter(|(name, _, _)| wanted.is_none_or(|asked| asked == name))
        .collect();
    if shown.is_empty() {
        let asked = wanted.map_or_else(String::new, Clone::clone);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "words: this build speaks no vocabulary called {asked:?}. It speaks {}.",
                known
                    .iter()
                    .map(|(name, _, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
    }
    for (name, answers, said) in shown {
        // The NAME and what it answers on one line, the words on the next and indented — so a
        // reader gets the question before the vocabulary, and `sprag words verdict | tail -1` is
        // the whole list for a script.
        println!("{name}  — {answers}");
        println!("  {}", said.join(" "));
    }
    Ok(())
}

/// One line per ending this build can record: the ENDING, what happens NEXT, and the sentence that
/// says what that means — register item 867.
///
/// # ⚠⚠⚠ The FIRST FIELD IS THE ENDING'S WORD, and `.githooks/loop-read.sh` depends on it
///
/// That hook reads the daemon's run log off disk at push time, holds an outcome word per finished
/// run, and looks the word up in this table — the whole point being that it keeps no copy of the
/// mapping. So the column order is a CONTRACT, not a layout: the leading spaces make the rows a
/// person can scan, the first field is the word to match on, and everything after it is printed
/// verbatim by whoever matched. `cli.rs`'s `the_push_time_reader_says_what_happens_next_…` holds
/// the two ends of that together by running the real hook against this real binary.
///
/// ⚠ The pairing is asked of [`Disposition::table`](sprag_plugin::driver::Disposition::table) and
/// never spelled here — the rule `outcome_word` states and the defect items 855 and 864 each
/// paid for: a renderer with its own opinion is a second authority on a set `sprag_plugin` owns.
fn disposition_rows() -> Vec<String> {
    sprag_plugin::driver::Disposition::table()
        .map(|(word, next)| {
            // ⛔⛔⛔⛔⛔ THE THIRD COLUMN IS WHAT A MACHINE MAY DO ALONE — register item 872(2), and
            // it is a WORD in the row rather than a sentence inside the last column for one
            // reason: the reader this column is for is not a person. An executor matches on
            // fields, and a tally of the arms saying a machine may not proceed alone was prose in
            // three files that nothing could match on. ⚠ It is appended rather than inserted,
            // because the first two fields are a CONTRACT `.githooks/loop-read.sh` matches on.
            //
            // ⛔⛔⛔⛔⛔ AND THE FOURTH IS WHAT IS TO DO IT — register item 872(1). The third says
            // whether a machine MAY and with what brief; this says WHICH PARTY IS OWED the next
            // run, and the two cross: `person` and `nothing` both answer `never` there and answer
            // differently here, `same_work` and `next_work` the other way about. Item 872 measured
            // 22 endings that permitted a next run and got none — permission was never the gap,
            // and a reader holding only the third column cannot say who failed to act because
            // nothing had named anybody. ⚠ Appended for the third column's reason exactly.
            format!(
                "  {:<10}  {:<9}  {:<10}  {:<16}  {} — {} — {}",
                word,
                next.wire_str(),
                next.unattended().wire_str(),
                next.opens_next().wire_str(),
                next.describe(),
                next.unattended().describe(),
                next.opens_next().describe(),
            )
        })
        .collect()
}

/// ⛔⛔⛔⛔⛔ `disposition [OUTCOME]`: **WHAT HAPPENS NEXT TO A RUN THAT ENDED, ASKED OF THIS
/// BUILD** — register item 867, and the second half of item 827.
///
/// # ⚠⚠⚠⚠⚠ What item 827 left, and what this is for
///
/// 827 moved six endings' *and now what* out of doc comments and into
/// [`Disposition`](sprag_plugin::driver::Disposition), and made `runs` print it for a run somebody
/// asks about. It could not make anybody who is NOT asking about one particular run able to ask at
/// all — and the reader that matters is exactly that one: `.githooks/loop-read.sh` runs at push
/// time, with no daemon, and prints the endings nobody has read. It held a word per run and had
/// nowhere to send it. Item 867 measured the alternative and refused it: writing the mapping into
/// the script is the *"one value, two homes"* defect items 855 and 864 each paid for.
///
/// So the product publishes the table and the script relays what it says. It needs **no daemon**
/// (`words`' reason: the classification is compiled in) and **no source tree**.
///
/// # ⛔ AN UNKNOWN OUTCOME IS A REFUSAL THAT NAMES WHAT THERE IS
///
/// This workspace's rule 6, and [`Disposition::of_outcome_word`](sprag_plugin::driver::Disposition::of_outcome_word)
/// says it in its own doc: a word nothing has classified is a RED, not a pass. A caller who asked
/// about `panicked` and got silence would read it as *nothing to do* — which is precisely the state
/// item 827 was filed on, invented rather than recorded.
///
/// # ⚠ It prescribes NOTHING, and that is item 827's own prohibition carried forward
///
/// Nothing here starts, stops or schedules a run. WHICH endings a machine may proceed past is a
/// question this table now answers in its own third column
/// ([`Unattended`](sprag_plugin::driver::Unattended), register item 872(2)) rather than in a
/// sentence here — *reading this table* and *obeying it* are different acts and only the first one
/// is here.
///
/// ⚠⚠ **AND THE FOURTH COLUMN NAMES A PARTY, WHICH IS STILL NOT A PRESCRIPTION** —
/// [`Opener`](sprag_plugin::driver::Opener), register item 872(1). It says who is OWED the next
/// run off an ending, not that anything opens one; this verb opens nothing and neither does the
/// crate the answer comes from. Item 872 measured endings that permitted a next run and got none,
/// and the gap it names is that no party was on record, so nobody had failed to act.
///
/// ⛔⛔ This paragraph used to TALLY THE ARMS and say a machine may not proceed past that many of
/// them — a number stated in prose, in three separate files, with nothing anywhere reading any of
/// them. A fifth disposition would have left all three silently wrong. The column is what an
/// executor matches on and what a gate can hold; the tally is gone from all three by a gate that
/// reads their sources.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for an outcome word nothing classifies, and for a second
/// argument.
fn disposition(args: Vec<String>) -> io::Result<()> {
    if let Some(extra) = args.get(1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "disposition: unexpected argument {extra:?} (it takes [OUTCOME], one at a time)"
            ),
        ));
    }
    let rows = disposition_rows();
    // ⚠ THE FILTER IS THE ONLY BRANCH — `words`' rule, for `words`' reason: printing every row and
    // printing one are the same act over a different set, so a named OUTCOME cannot come to be
    // formatted differently from the whole. The hook that parses these rows reads both forms.
    let wanted = args.first();
    let shown: Vec<&String> = rows
        .iter()
        .filter(|row| {
            wanted.is_none_or(|asked| row.split_whitespace().next() == Some(asked.as_str()))
        })
        .collect();
    if shown.is_empty() {
        let asked = wanted.map_or_else(String::new, Clone::clone);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "disposition: nothing in this build classifies what happens next after an ending \
                 spelled {asked:?}. The endings it classifies are {}.",
                sprag_plugin::driver::Disposition::table()
                    .map(|(word, _)| word)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
    }
    println!(
        "disposition  — what happens next to a run that ended this way. It says what to do, and \
         does none of it"
    );
    for row in shown {
        println!("{row}");
    }
    Ok(())
}

/// 🎯🎯🎯🎯🎯 `waits [LOG]`: **HOW LONG EACH WORKING TREE HAD NOTHING DRIVING IT** — register item
/// 872 ⑶, and the command item 827's number has never had.
///
/// # ⛔⛔⛔⛔⛔ The clause this answers, and why it stood open through four re-judgements
///
/// Item 827 measured **3 h 49 m** between a loop run dying and the next one being launched. It
/// measured it by hand, once. Item 872 ⑴⑵ put on record who is owed the next run off each ending
/// and which endings a machine may never proceed past, and ⑶ is *measure the delay again* — which
/// four rounds each judged blocked, three of them for reasons the fourth refuted. Item 888 built
/// both ends of the interval expressly for this clause and **no surface ever read them**.
///
/// ⇒ So the only way to answer ⑶ was a `python3 -c` over the store, which is this workspace's rule
/// 10 exactly: a number nothing computes gets taken once and quoted until it is wrong. That is what
/// this replaces.
///
/// # ⛔⛔⛔⛔⛔ IT PRINTS WHAT IT CANNOT MEASURE, AND THAT IS THE POINT
///
/// A run this cannot pair yields no stretch, so a report of stretches alone says *nothing to see*
/// for a store in which nothing is measurable at all — and **that is today's store**: measured
/// 2026-09-05T07:45:30Z, 228 rows carrying `ran_from` 0 times, `ran_to` 0 times and `tree` never.
/// The live daemon predates all three columns. So the unmeasured half is printed as loudly as the
/// measured one, under [`sprag_host::runs::NoWait`]'s own words, and a reader can tell *the promotion
/// has not happened yet* from *the handovers were quick*.
///
/// ⚠⚠ **NEEDS NO DAEMON**, and not for convenience: the delay is bounded at its left end by a
/// daemon that is GONE, so a verb that required a live one could never answer about the promotion
/// that ended it. It reads the logs `crate`'s durability layer leaves on disk — item 867's reader,
/// `.githooks/loop-read.sh`, works from the same DERIVATION for the same reason.
///
/// ⚠ Naming a LOG reads exactly that file, which is how a test drives this over a store it wrote.
/// Naming none SWEEPS every `*.runs.json` in sprag's state directory, which is what a person asking
/// about this machine means.
///
/// # ⛔⛔⛔⛔⛔ AND A SWEPT ANSWER NAMES ITS DIRECTORY, because the derivation moves
///
/// *Same derivation* is not *same directory*, and this doc said the second until it was measured.
/// The loop exports its own `XDG_STATE_HOME`; a push-time hook inherits none. **Measured
/// 2026-09-05T08:47:23Z**: this verb answered about the loop's **229 runs** with the variable set
/// and about **62 integration-test runs across twelve logs** without it — same command, same
/// machine, both tables long and confident, and nothing said which. So the sweep's address is
/// printed with it; see [`waits_lines`], which holds the argument.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a second argument, and [`io::ErrorKind::NotFound`] when the
/// named log cannot be read or the state directory holds none — ⛔ a REFUSAL rather than an empty
/// table, on this workspace's rule 6: *no logs* and *no waiting* must never print alike.
fn waits(args: Vec<String>) -> io::Result<()> {
    let (read, dir) = run_logs("waits", "nothing waited", &args)?;
    println!(
        "waits  — how long each working tree had NO run driving it, between one run's watched stop \
         and the next one's watched start"
    );
    // ⚠ The swept directory travels so the ANSWER can name it — see `waits_lines`. `None` when a
    // caller named a log, because then the caller already said where and a warning about a
    // derivation that did not happen would make the certain case read as the doubtful one.
    for line in waits_lines(&read, args.is_empty().then_some(dir.as_path())) {
        println!("{line}");
    }
    Ok(())
}

/// [`waits`]'s BODY, separated from the printing so a gate can read what it says — the split
/// [`disposition_rows`] makes one verb over, and for its reason: the mouth is where item 856 ⑸
/// measured a value crossing into nothing, and a renderer nothing drives is a renderer that goes
/// green while saying anything at all.
fn waits_lines(
    read: &[(std::path::PathBuf, sprag_host::runs::RunLog)],
    swept: Option<&std::path::Path>,
) -> Vec<String> {
    let mut lines = Vec::new();
    // ⛔⛔⛔⛔⛔ **WHICH DIRECTORY THIS IS AN ANSWER ABOUT, WHEN NOBODY NAMED ONE** — and it is here
    // because running the verb without `XDG_STATE_HOME` printed a confident, complete, WRONG answer.
    //
    // **Measured 2026-09-05T08:47:23Z.** The loop exports its own `XDG_STATE_HOME`; a push-time hook
    // inherits none. With it, this verb reads the loop's store — 229 runs. Without it, it reads
    // `~/.local/state/sprag` and reported **62 runs across twelve logs**, every one of them left by
    // an integration test (`sprag-cli-it-*`, `sprag-fold`, `sprag-host`). Same command, same
    // machine, two answers, and nothing in the output said which.
    //
    // ⚠⚠ `.githooks/loop-read.sh` carries this hazard as a named sentence (`LOOP_READ_BLIND_COST`)
    // for the same directory and the same reason — *the fallback is a real directory holding real
    // logs from integration tests*. It is SHARPER here: there the wrong place prints zeros, which
    // reads as *nothing happened*; here it prints a full table, which reads as *this is your
    // machine*. Item 790's lesson at one more remove.
    //
    // ⚠ Said whether or not anything was found, because the sweep's ADDRESS is what the reader has
    // to be able to doubt — a table is not more trustworthy for being long.
    if let Some(dir) = swept {
        lines.push(format!(
            "swept {} — this path is derived from XDG_STATE_HOME and MOVES with it; the fallback \
             holds integration-test logs, so a table from the wrong place looks exactly like a \
             table from the right one",
            dir.display()
        ));
    }
    // ⛔⛔⛔ EMPTY LOGS ARE COUNTED, NOT LISTED — and this line is here because running the verb at
    // the real store printed it: a machine that has served TUI clients accumulates a `*.runs.json`
    // per socket, and at 2026-09-05T07:52:53Z that was **68 empty logs against 1 with runs in it**,
    // so the single line carrying the answer sat under sixty-eight carrying nothing.
    //
    // ⚠⚠ The COUNT stays, because *sixty-eight logs held nothing* and *there was one log* are
    // different facts and a reader must not infer the first from an absence — item 856's rule
    // pointing the other way. What is dropped is the repetition, never the population.
    let mut empty = 0usize;
    for (path, log) in read {
        let waits = log.waits_between_runs();
        if waits.runs() == 0 {
            empty += 1;
            continue;
        }
        lines.push(format!("{}  {} run(s)", path.display(), waits.runs()));
        for wait in &waits.measured {
            lines.push(format!(
                "  {} waited {}s after run {} until run {}",
                wait.tree, wait.seconds, wait.after, wait.before
            ));
        }
        // ⛔⛔⛔⛔⛔ AND WHAT COULD NOT BE MEASURED, AS LOUDLY. A report of stretches alone reads
        // *nothing to see* for a store in which nothing is pairable, and that is today's store
        // exactly: 229 runs, 229 of them behind a wall. The empty ARMS are dropped here at the
        // mouth — item 856's `0 of 0` rule — and never in the answer, which carries all six.
        for (why, count) in waits.unmeasured.iter().filter(|(_, count)| *count > 0) {
            lines.push(format!(
                "  {count} run(s) measure nothing: {}",
                why.describe()
            ));
        }
        // ⛔⛔⛔⛔⛔ AND THE SECOND AXIS, because the first one stops at the first wall. Every
        // reason above `TreeUnknown` presumes a grouping, so a log whose rows carry no tree is
        // told entirely in that one word — and **that is this machine's store**: measured
        // 2026-09-05T12:11:19Z, 231 rows, 231 of them `TreeUnknown`, and separately 0 of them
        // carrying the watched stop a stretch is measured FROM. Printing only the first invites
        // the reading *backfill item 890's column and the number appears*, which is false: no
        // grouping can make a left end out of a run nobody watched stop.
        //
        // ⚠ Said for every log with runs in it, including one where the count is the whole
        // population — *all of them can be a left end* is what makes a report of no stretches mean
        // the successors are missing rather than the ends.
        let rest: Vec<String> = waits
            .left_ends
            .iter()
            .filter(|(arm, count)| *arm != sprag_host::runs::LeftEnd::Watched && *count > 0)
            .map(|(arm, count)| format!("{count} {}", arm.describe()))
            .collect();
        lines.push(format!(
            "  {} of {} run(s) carry the watched stop a stretch is measured from{}",
            waits.watched_left_ends(),
            waits.runs(),
            if rest.is_empty() {
                String::new()
            } else {
                format!(" — {}", rest.join(", "))
            },
        ));
    }
    if empty > 0 {
        lines.push(format!("{empty} further log(s) held no runs at all"));
    }
    lines
}

/// **THE RUN LOGS A `[LOG]` VERB IS ANSWERING ABOUT** — [`waits`]' reader and [`folds`]', written
/// once.
///
/// # ⚠⚠ Why the two verbs share this rather than each carrying its own copy
///
/// They ask different questions of the same files and must not come to disagree about WHICH files
/// those are. Three decisions live here and each is one this workspace has paid for: naming a LOG
/// reads exactly that file; naming none SWEEPS `*.runs.json` out of sprag's state directory; and a
/// log this build cannot parse is SKIPPED rather than guessed at — which is why nothing readable
/// is a REFUSAL and not an empty table, on rule 6, since *no logs* and *nothing to report* must
/// never print alike. A second spelling is a second place for any of the three to be softened.
///
/// It hands back the derived directory as well, so a caller can name the sweep's address in the
/// answer — see [`waits_lines`], which holds the argument for why that matters.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a second argument, and [`io::ErrorKind::NotFound`] when the
/// named log cannot be read or the state directory holds none.
fn run_logs(
    verb: &str,
    nothing_happened: &str,
    args: &[String],
) -> io::Result<(
    Vec<(std::path::PathBuf, sprag_host::runs::RunLog)>,
    std::path::PathBuf,
)> {
    if let Some(extra) = args.get(1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{verb}: unexpected argument {extra:?} (it takes [LOG], one at a time)"),
        ));
    }
    let dir = sprag_host::state_dir();
    let logs: Vec<std::path::PathBuf> = match args.first() {
        Some(named) => vec![std::path::PathBuf::from(named)],
        // Sorted so two readings of one machine are diffable — the whole use of these verbs is
        // comparing today's number with a number somebody wrote down.
        None => {
            let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|name| name.ends_with(".runs.json"))
                })
                .collect();
            found.sort();
            found
        }
    };
    let read: Vec<(std::path::PathBuf, sprag_host::runs::RunLog)> = logs
        .into_iter()
        .filter_map(|path| sprag_host::load_runs(&path).map(|log| (path, log)))
        .collect();
    if read.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{verb}: no readable run log{}. A log this build cannot parse is skipped rather \
                 than guessed at, so an empty answer here means NOTHING WAS READ and never that \
                 {nothing_happened}.",
                match args.first() {
                    Some(named) => format!(" at {named}"),
                    None => format!(" in {}", dir.display()),
                }
            ),
        ));
    }
    Ok((read, dir))
}

/// 🎯🎯🎯🎯🎯 `folds [LOG]`: **HOW FULL EACH SESSION WAS WHEN IT FOLDED THE PROMPTS IT WAS SENT**
/// — register item 856 ⑴, and the command that item's number has never had.
///
/// # ⛔⛔⛔⛔⛔ The clause this answers, and what five re-judgements each did instead
///
/// Item 856's axis is *a session folds because of how full it is*, and its refutation is stated by
/// the loop itself: **one `capacity` reflection whose prompt LANDS**. Measured 2026-09-05 that had
/// happened 29 times — and every one of the 29 came from a run whose ceiling a caller had moved to
/// `20000`, where a `capacity` reflection means *we handed over early* rather than *the session
/// filled up*. The condition had been assuming **ceiling = fullness**, and telling the two apart
/// takes three columns that only exist together in a row: how full it got (item 894), what it was
/// judged by (856 ⑴b) and whose numbers those were (859).
///
/// ⇒ Nothing read the three together, so the answer was a `python3 -c` at the store, re-typed each
/// round — this workspace's rule 10, and the same absence [`waits`] was written for one item over.
/// The experiment's own arms had to be separated from the ordinary runs **by reading a human note
/// in a memory file**, which is the step this verb exists to end.
///
/// # ⛔⛔⛔⛔⛔ IT PRINTS WHAT IT CANNOT READ, AND TODAY THAT IS THE WHOLE ANSWER
///
/// A run this cannot read yields no row, so rows alone would say *nothing to see* about a store
/// where nothing is readable — **and that is today's store**: measured 2026-09-05T10:13:40Z, 229
/// rows with `context_high_water` 0 times, `context_ceiling` 0 times, `overridden` 0 times. The
/// daemon driving that loop predates all three. So the unreadable half prints as loudly as the
/// readable one, under [`sprag_host::runs::NoFullness`]'s own words, and *the promotion has not
/// happened yet* cannot be read as *nothing folded*.
///
/// ⚠⚠ **NEEDS NO DAEMON**, and not for convenience: item 856's rate is over runs that have ENDED,
/// and item 606 measured thirteen live runs of which every single one was a RESTORED record. The
/// population this answers about exists only in a file.
///
/// # Errors
///
/// [`run_logs`]'s, verbatim.
fn folds(args: Vec<String>) -> io::Result<()> {
    let (read, dir) = run_logs("folds", "nothing folded", &args)?;
    println!(
        "folds  — how full each session was when it folded the prompts it was sent, and whose \
         ceiling it was judged by"
    );
    // ⚠ The swept directory travels so the ANSWER can name it — `waits_lines` holds the argument,
    // and `None` when a caller named a log for the same reason it gives.
    for line in folds_lines(&read, args.is_empty().then_some(dir.as_path())) {
        println!("{line}");
    }
    Ok(())
}

/// [`folds`]'s BODY, separated from the printing so a gate can read what it says — [`waits_lines`]'
/// split, for its reason: the mouth is where item 856 ⑸ measured a value crossing into nothing.
fn folds_lines(
    read: &[(std::path::PathBuf, sprag_host::runs::RunLog)],
    swept: Option<&std::path::Path>,
) -> Vec<String> {
    let mut lines = Vec::new();
    // ⛔ WHICH DIRECTORY THIS IS AN ANSWER ABOUT — `waits_lines` carries the measurement this line
    // exists for (2026-09-05T08:47:23Z, the same command answering about two machines' worth of
    // runs depending on one environment variable).
    if let Some(dir) = swept {
        lines.push(format!(
            "swept {} — this path is derived from XDG_STATE_HOME and MOVES with it; the fallback \
             holds integration-test logs, so a table from the wrong place looks exactly like a \
             table from the right one",
            dir.display()
        ));
    }
    let mut empty = 0usize;
    for (path, log) in read {
        let folds = log.folds_against_fullness();
        if folds.runs() == 0 {
            empty += 1;
            continue;
        }
        lines.push(format!("{}  {} run(s)", path.display(), folds.runs()));
        for row in &folds.measured {
            // ⛔ THE EMPTY OCCASIONS ARE DROPPED HERE AT THE MOUTH and never in the answer, which
            // carries all of `Occasion::ALL`: `0 of 0` on a road nothing was ever asked on reads as
            // *clean* — item 856's own rule about a table whose zeros are indistinguishable.
            let split: Vec<String> = row
                .folds
                .rows()
                .filter(|(_, under)| under.delivered > 0)
                .map(|(occasion, under)| {
                    format!(
                        "{} {} of {}",
                        occasion.word(),
                        under.folded,
                        under.delivered
                    )
                })
                .collect();
            lines.push(format!(
                "  run {}: {} read of a {} ceiling, {}{} — {}",
                row.id,
                row.fullest,
                row.ceiling,
                row.judged.describe(),
                // ⚠ A row whose peak is BELOW the ceiling **it reflected on** is a defect in the
                // recording rather than a fact about a session — the document turns on
                // `context >= context_ceiling` and the peak is taken over those readings. Said
                // rather than dropped: a reader comparing the two columns must be able to see them
                // disagree.
                //
                // ⛔⛔⛔⛔⛔ THE EMPHASIS IS THE FIX. This asked `reached_its_ceiling` alone, and
                // on 2026-09-05T13:01:55Z it printed that warning over run 232 — the FIRST
                // ordinary run this item ever got a fullness from, whose capacity road was
                // untaken. A run that has not reflected is supposed to sit below its ceiling.
                // `columns_disagree` asks both halves, so the mouth cannot hold half the rule.
                if row.columns_disagree() {
                    " ⚠ ITS PEAK IS BELOW THAT CEILING, so the two columns disagree".to_owned()
                } else {
                    String::new()
                },
                if split.is_empty() {
                    "no prompt was asked on any road".to_owned()
                } else {
                    split.join(" · ")
                },
            ));
        }
        // ⛔⛔⛔⛔⛔ THE TWO LANDING COUNTS, BOTH OR NEITHER, AND NEVER ADDED. The first is the
        // axis's own stated refutation and the second is what an EXPERIMENT bought; printing only
        // the first would hide the 29 that were quoted as a refutation for a day, and printing
        // their sum would refute the axis with the experiment's own definition of *full*.
        //
        // ⛔⛔⛔ **AND NEITHER IS PRINTED OVER AN EMPTY POPULATION** — item 856's `0 of 0` rule,
        // which this verb's first run at the real store made the case for: `0 landing(s) refute
        // the axis` above a store where NOTHING is readable reads as *the axis survived*, and the
        // register's own repeated failure is a zero being read as clean. When no run can be read,
        // the population is said instead and the counts are withheld.
        if folds.measured.is_empty() {
            lines.push(
                "  no run here can be read against a fullness, so the landing counts have NO \
                 population and are withheld — see the reasons below, not a zero"
                    .to_owned(),
            );
        } else {
            lines.push(format!(
                "  {} capacity landing(s) refute the axis — a prompt asked of a session judged by \
                 its OWN document's ceiling, and not folded",
                folds.refutations(),
            ));
            lines.push(format!(
                "  {} further landing(s) are at a ceiling a caller moved, which is a different \
                 sentence and is not added to the line above",
                folds.landings_at_a_moved_ceiling(),
            ));
        }
        // ⛔⛔⛔⛔⛔ AND WHAT COULD NOT BE READ, AS LOUDLY — today that is every run in the store.
        for (why, count) in folds.unmeasured.iter().filter(|(_, count)| *count > 0) {
            lines.push(format!(
                "  {count} run(s) measure nothing: {}",
                why.describe()
            ));
        }
    }
    if empty > 0 {
        lines.push(format!("{empty} further log(s) held no runs at all"));
    }
    lines
}

/// ⛔⛔⛔⛔⛔ **WHICH DAEMONS ARE RUNNING, AND WHERE** — register item 825, and the verb that
/// exists so a LAUNCHER never has to guess a socket path again.
///
/// # ⚠⚠⚠⚠⚠ The sentence this replaces was true and sent its reader to the wrong machine
///
/// The owner pressed the dock icon six times over six days. Each press asked the well-known socket,
/// found the file a daemon had left behind on 2026-08-25, and put *"no server running at
/// `/run/user/1000/sprag-host.sock`"* on screen — while a daemon served six windows on
/// `/run/user/1000/sprag-loop.sock`. **Nothing it printed was false.** What it lacked was the
/// question this verb asks.
///
/// # ⚠⚠⚠ Every socket gets a WORD, and there is no fourth
///
/// [`Answered`](sprag_rpc::survey::Answered) is closed — `serving`, `refused`, `silent` — and each
/// one is a different repair. The launcher's own header had two of them written down before this
/// round (*a daemon that is not running and a daemon that is too old are different problems*); the
/// third is *a daemon that is running somewhere else*, which is what item 825 measured.
///
/// ⚠⚠ **THE POPULATION IS PRINTED EVEN WHEN IT IS EMPTY.** A reader told *no daemon* must be able
/// to tell that from *it did not look where mine is* — a daemon whose operator pointed
/// `SPRAG_HOST_RPC_SOCK` outside this product's own naming is not asked about, and the last line
/// says so by naming the directory and the pattern rather than leaving silence to be read as
/// absence.
///
/// ⚠ `--serving` prints the serving sockets and nothing else, one per line: it is what a script
/// consumes, so no caller has to parse a sentence written for a person.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for any argument other than `--serving`. A machine with no
/// daemon on it is not an error of this kind — it EXITS 1 after printing every socket's word, so a
/// caller branches on the status and a reader still gets the population.
fn daemons(args: Vec<String>) -> io::Result<()> {
    let mut only_serving = false;
    for arg in &args {
        if arg == "--serving" {
            only_serving = true;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("daemons: unexpected argument {arg:?} (it takes [--serving])"),
            ));
        }
    }
    let under = sprag_rpc::survey::runtime_dir();
    // ⚠ THE SAME ID EVERY OTHER COMMAND OF THIS PROCESS USES, so a daemon's log shows the survey as
    // the client it was rather than as an anonymous connect it cannot account for.
    let found = sprag_rpc::survey::survey(&under, &cli_client_id(), CONNECT_TIMEOUT);
    let serving = found.serving();
    if only_serving {
        for path in &serving {
            println!("{}", path.display());
        }
    } else {
        for row in &found.asked {
            println!("{}  {}", row.path.display(), row.answer);
        }
        // ⚠⚠ ALWAYS, including when something WAS found: the population is what makes the answer
        // readable, and a line that appears only on failure is one nobody has read when it matters.
        println!(
            "asked {} socket(s) matching {} under {}",
            found.asked.len(),
            sprag_rpc::survey::Survey::pattern(),
            under.display(),
        );
    }
    if serving.is_empty() {
        // ⚠ A REFUSAL EXIT, because the caller asked a yes/no question and a script must be able to
        // branch without reading the lines. The lines above have already said WHY for each socket.
        std::process::exit(1);
    }
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
    // `-N` is tmux's own flag for "the readable form", and this takes it for the same reason: the
    // paste-back shape below is a contract (every line after the first is a command), so the view a
    // person reads has to be a SECOND form rather than a change to the first. The rows are the
    // frontends' own — [`sprag_host::keyhelp`] — so the three surfaces cannot come to say different
    // things about one table, and this one needs no daemon and no terminal.
    let notes = args.first().is_some_and(|arg| arg == "-N");
    let unexpected = if notes { args.get(1) } else { args.first() };
    if let Some(unexpected) = unexpected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("list-keys: unexpected argument {unexpected:?} (it takes [-N])"),
        ));
    }
    let keymap = sprag_host::config::keymap().map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("list-keys: {error}"))
    })?;
    if notes {
        let help = sprag_host::keyhelp::KeyHelp::of(&keymap);
        for row in help.rows() {
            // The COLUMN is this surface's decision and the WORDS are not: a binding gets its chord
            // padded out so the actions line up in a pipe, and everything else prints the row's own
            // text. Padded in CHARACTERS, exactly as the paste-back form below is and for the same
            // reason — a key spec is a user's string, and `%` is not the only thing anyone binds.
            match row {
                sprag_host::keyhelp::Row::Bind {
                    chord,
                    action,
                    repeat,
                } => {
                    let pad = " ".repeat(help.chord_width().saturating_sub(chord.chars().count()));
                    let mark = if *repeat {
                        sprag_host::keyhelp::KeyHelp::REPEAT
                    } else {
                        "  "
                    };
                    println!("  {chord}{pad}  {mark} {action}");
                }
                sprag_host::keyhelp::Row::Blank => println!(),
                sprag_host::keyhelp::Row::Heading(_) => println!("{row}"),
                sprag_host::keyhelp::Row::Vocabulary { .. } => println!("  {row}"),
            }
        }
        return Ok(());
    }
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
            BoundAction::vocabulary().join(", ")
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

/// What `sprag` with no verb (or an unknown one) prints — the whole verb set in one place.
///
/// **BUILT from the vocabulary since R323**, where it used to be a `const` beside this function
/// whose own doc said *"a second list is exactly what nothing checks"*. It was right: measured by
/// running the shipped binary, `run` and `hook` were dispatched and named in it nowhere. Both
/// halves now come off [`sprag_host::vocabulary::usage`], which iterates the same array
/// [`dispatch`] is exhaustive over.
fn print_usage() {
    eprintln!("{}", vocabulary::usage());
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
///
/// # ⚠⚠⚠⚠ It names the COMMIT now, and it still asks no daemon — both halves are decisions
///
/// `CARGO_PKG_VERSION` is `0.0.1` and has never moved, so *"which build is this"* had an answer
/// that was the same sentence for every build this repository has ever produced. The commit is the
/// part that distinguishes them ([`sprag_host::wire::BUILD`]).
///
/// ⚠⚠⚠ **And this is THIS BINARY, never the daemon's** — which is the trap register item 438 was
/// filed for. The client is rebuilt every round and the daemon is not, so a `--version` that
/// printed one number would be read as naming both, and it would name the one that was never in
/// doubt. Contacting the daemon here is refused for the reason above (a version that only answers
/// while the server is healthy cannot answer the question it exists for), so the PAIR is
/// [`doctor`]'s to report — a command that already requires a daemon and whose whole job is saying
/// what is wrong with the machine.
/// ⚠⚠ **THE SENTENCE IS COMPOSED IN ONE PLACE** — register item 897. It was spelled here while
/// this was the only image that could say anything at all; three more say it now, and four
/// spellings of a shape the promotion door PARSES is three chances to drift somewhere a person
/// would not notice.
fn print_version() {
    println!("{}", sprag_host::promotion::version_line("sprag"));
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

/// Env handed to a client naming WHICH session to adopt — [`attach`]'s whole mechanism.
///
/// Spelled here rather than imported from `sprag-client`: this CLI does not depend on that crate,
/// and the two spellings agreeing is a fact worth a test rather than a fact hidden behind a `use`.
const GUI_SESSION_ENV: &str = "SPRAG_GUI_SESSION";

/// Env handed to a client that must create a session of its OWN rather than adopt — [`new`]'s `-a`.
///
/// The COMPLEMENT of [`GUI_SESSION_ENV`], and the pair is exhaustive over what a launch can mean:
/// this one names no session and refuses to adopt, that one names exactly which to adopt, and
/// neither present means *take me to my work* (register item 284).
const GUI_NEW_ENV: &str = "SPRAG_GUI_NEW";

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
/// # The deadline is stated BEFORE the connection exists, and that is the point
///
/// This function performs I/O of its own (the handshake), so a caller cannot bound it afterwards —
/// by the time it holds a `HostConn` the unbounded read has already happened. That is not
/// hypothetical: adding the handshake without a deadline parameter re-opened the exact hole R273
/// closed, and `a_wedged_daemon_cannot_stall_the_agents_hook` caught it.
///
/// # ⚠ Waiting forever is the EXCEPTION now, and it has to be spelled
///
/// This took `Option<Duration>` and 26 of its 27 callers passed `None`, on a rationale written into
/// [`events`]: *"every other verb here is a request-response against a local daemon answering from
/// memory, so a reply that has not arrived in seconds is not slow, it is not coming."* That
/// sentence was true and nothing enforced it — the default was *forever*, and each new call site
/// got it by saying nothing.
///
/// R343 measured what that costs. `sprag-peer` dropped every connection it accepted on macOS, and
/// two CLI runs in `tests/cli.rs` — with no person there to interrupt them — waited **3 h 38 min**,
/// until GitHub killed the job. The suite reported nothing at all, which is why five rounds had
/// recorded macOS as *unmeasured* rather than as *broken*.
///
/// So the bound is the default and the exception is named: a verb whose contract is to PARK
/// ([`wait_for_output`], [`events`]) clears it with `set_read_deadline(None)` at the moment it
/// parks — which `wait_for_output` already did, against a default that was not yet there.
fn connect() -> io::Result<HostConn> {
    connect_within(REQUEST_DEADLINE)
}

/// [`connect`] with a tighter bound than a person's command wants — for the paths that run inside
/// somebody else's process while that process waits.
fn connect_within(deadline: Duration) -> io::Result<HostConn> {
    let endpoint = HostEndpoint::for_opts(HOST_SOCKET);
    let mut conn = HostConn::connect(endpoint.path(), CONNECT_TIMEOUT).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no server running at {endpoint}"),
        )
    })?;
    // Set BEFORE the handshake, so the very first reply this process waits for is covered.
    conn.set_read_deadline(Some(deadline))?;
    conn.handshake(&cli_client_id())?;
    // Where this process is, asked once and before any command has shaped a request — every params
    // builder below consults the answer, and a cache filled halfway through a command would make
    // two requests of one command mean two different sessions.
    learn_where_we_are(&mut conn);
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
    let mut conn = connect()?;
    let sessions = query_slot(&mut conn, json!({ "path": mux_action_path(SESSIONS_SLOT) }))?;
    // Best-effort: a daemon too old to serve the family leaves every line in its structural form
    // rather than failing a listing (`sprag ls` answers "what may I name?" first and foremost). The
    // wire protocol makes that skew a refusal at the door, so this is belt to that suspenders.
    let activity = query_slot(
        &mut conn,
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

/// `list-clients [-t SESSION]`: one line per ATTACHED client — its opaque id, the session it is
/// viewing and the WINDOW of that session it is on — tmux `list-clients`. With `-t SESSION`, only
/// clients attached to that session (the session is pre-flighted so a typo is a clean error, like
/// the window commands). The client id is what a `sprag-gui` window mints (`gui-{pid}-{nanos}`); the
/// daemon has no tty to report, so the row is the honest subset of tmux's `struct client`.
///
/// # Why the window is on the row
///
/// It was not, and did not need to be: every client of a session saw the same one, so the column
/// would have repeated the session's own answer. R346 made a view a fact about the CLIENT, and the
/// first question a person asks when their panes are the wrong size is *who else is on this window*
/// — which the size arbitration now folds per window and this listing is the place to answer.
/// Omitted rather than faked for a client whose window has gone, the same rule as the area beside
/// it.
fn list_clients(args: Vec<String>) -> io::Result<()> {
    let filter = optional_target(args, "list-clients")?;
    let mut conn = connect()?;
    if let Some(session) = &filter {
        require_session(&mut conn, session)?;
    }
    let clients = query_slot(&mut conn, json!({ "path": mux_action_path(CLIENTS_SLOT) }))?;
    for client in clients.as_array().into_iter().flatten() {
        let id = client["client"].as_str().unwrap_or("?");
        let session = client["session"].as_str().unwrap_or("?");
        if filter.as_deref().is_some_and(|want| want != session) {
            continue;
        }
        // The area the client reported, in tmux's own `[COLSxROWS]` shape. Omitted rather than
        // faked when it has not reported one: this is what `window-size` arbitrates over, so a
        // client that is not in the arbitration must not read as though it were.
        let where_it_is = match client["window"].as_str() {
            Some(window) => format!("{session}:{window}"),
            None => session.to_owned(),
        };
        match (
            client["size"]["cols"].as_u64(),
            client["size"]["rows"].as_u64(),
        ) {
            (Some(cols), Some(rows)) => println!("{id}: {where_it_is} [{cols}x{rows}]"),
            _ => println!("{id}: {where_it_is}"),
        }
    }
    Ok(())
}

/// `sprag find NEEDLE [-t SESSION] [--pane N]` — search the session's current window and print each
/// matching line as `PANE:LINE: text`, the `grep -n` shape a script or an agent can slice.
///
/// **A SWEEP by default, not per-pane, on purpose.** The question a terminal user actually has is
/// "which pane has the error", so the sweep is the useful unit; `--pane` narrows it once the answer
/// to that question is known. An agent that already knows its pane uses the `find_in_pane` MCP tool
/// instead. None of the three implements a second search: all read the host's `find.<needle>`
/// family, so there is ONE definition of what matches (`sprag_vt::Screen::find`) and the CLI cannot
/// drift from the GUI's highlight.
///
/// ⚠⚠ **THE TWO FORMS DO NOT REACH THE SAME PANES, AND NARROWING REACHES FURTHER.** The sweep stops
/// at the scoped session's CURRENT WINDOW; `--pane` resolves anywhere in the session, because R312
/// made pane resolution session-wide for every verb and the sweep was deliberately left where it
/// was (see the comment at the call). So `find X --pane marked` can print a line that `find X` did
/// not — a filter that widens its own answer. This paragraph used to claim the sweep was
/// *"session-wide by default"* one line after the synopsis said *current window*, and both halves
/// of that pair could not be true; measured 2026-08-17, the synopsis was the honest one. Pinned by
/// `find_narrowed_to_a_pane_reaches_a_window_the_sweep_does_not`, open as register item 429.
///
/// A `--pane` naming a pane that is nowhere in the session is a clean ERROR, not an empty result:
/// the caller asked for a specific pane, and reporting "no matches" for a pane that is not there
/// would answer a question they did not ask. (It says *the session*, not *the current window*, for
/// the reach stated above — a pane one window over resolves and is searched.) Contrast the needle
/// itself, where finding nothing IS the answer. An invalid `--regex` pattern is an error for the
/// same reason — the search never ran, so exiting 0 with no output would claim it had.
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
    // Narrowed to ONE named pane, or every pane of the caller's own window. The narrowed form
    // resolves session-wide (a name reaches any window), and the sweep does not: searching every
    // window would change what an unnarrowed `sprag find` means for every caller that has one.
    let only = resolve_optional_pane(&mut conn, session.as_deref(), only.as_deref(), "find")?;
    let panes: Vec<(Option<String>, u64)> = match &only {
        Some(site) => vec![(site.window.clone(), site.id)],
        // ⚠⚠⚠⚠⚠ AND THE WINDOW STAYS `None` HERE, WHICH IS A MEASUREMENT AND NOT AN OVERSIGHT —
        // register item 759. This round wrote `here_window(session)` into this map and then asked
        // whether any run could reach it: **no.** This arm is entered only when
        // `resolve_optional_pane` answered `None`, and that happens under exactly the filter
        // [`here_window`] answers `None` under — *no `$SPRAG_PANE` of this scope*. The two are the
        // same condition, so a window here could never be anything but `None`, and code that
        // cannot be reached is worse than absent: it reads like a defence.
        //
        // ⇒ A caller standing in a pane never sweeps at all — it searches THAT pane, resolved one
        // line up. The sweep is the shell-outside-the-workspace form, and the daemon's current
        // window is the only answer it could mean.
        None => pane_ids(&mut conn, session.as_deref())?
            .into_iter()
            .map(|id| (None, id))
            .collect(),
    };
    let mut truncated = false;
    for (window, pane) in panes {
        let answer: Value = query_slot(
            &mut conn,
            windowed_params(
                session.as_deref(),
                pane_input_path(pane, &slot),
                window.as_deref(),
            ),
        )?;
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
        bad_input("wait-for-output: --pane is required (a wait names the pane it watches)")
    })?;
    let mut conn = connect()?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    // Resolved before the park for the reason the daemon checks it too: a wait on a pane that is
    // not there cannot be answered and cannot fail, so it would read as "it has not happened yet".
    // The park itself is SESSION-wide at the daemon (`handle_output_wait` checks the pane against
    // the session, not a window), so this only ever needed the name to resolve that far.
    let pane = resolve_pane(&mut conn, session.as_deref(), &pane, "wait-for-output")?.id;
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
    /// The one pane to search as the caller SPELLED it — an id or a NAME — or `None` to sweep the
    /// whole window. Unresolved here because resolution needs a connection, and this parse has none.
    pane: Option<String>,
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
                pane = Some(
                    it.next()
                        .ok_or_else(|| bad(format!("{verb}: --pane needs a pane id or NAME")))?,
                );
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

/// `new [name] [-a]`: create a session — born with a shell, tmux's `new-session -d` (the registry
/// allocates the lowest free name when none is given) — and print the name it got, the string to
/// scope a client to. The CLI passes no `cmd`/size, so the birth pane runs the default `$SHELL`.
///
/// # `-a` opens a WINDOW on a new session, and it is the reason this flag exists at all
///
/// A launch that names no session ADOPTS the work already there (register item 284) — tmux's shape,
/// and herdr's. That is right, and it costs a verb: *make me a new one* stops being what a bare
/// launch means, so without a word for it the only route to a fresh session would be a sidebar
/// button on a window already sitting in somebody else's work. `-a` is that word.
///
/// ⚠⚠ **The WINDOW creates the session, not this command**, and the difference is load-bearing:
/// a client births its first pane at the size IT measured (its own glyph metric and area), which is
/// what makes `gui-font` observable end to end. A session created HERE would be born at the
/// daemon's default and merely resized afterwards. So `-a` hands the client [`GUI_NEW_ENV`] and
/// gets out of the way, which is also why it prints no name: the window owns the session it made,
/// and the person is looking at it.
///
/// ⚠ A NAME with `-a` is REFUSED rather than quietly ignored. Naming a session the client has not
/// created yet would need a third meaning for [`GUI_SESSION_ENV`] (*create under this name*, beside
/// *attach to this one*), and a flag that accepted the name and dropped it would be the worse
/// answer. `sprag new NAME` then `sprag attach NAME` is the joined-up route today.
fn new(args: Vec<String>) -> io::Result<()> {
    let mut name = None;
    let mut window = false;
    let mut cwd: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-a" | "--attach" => window = true,
            // tmux's `new-session [-c start-directory]`, and item 417's half: the wire has taken a
            // `cwd` since the key existed and no CLI verb sent one.
            "-c" => {
                cwd = Some(
                    it.next()
                        .ok_or_else(|| bad_input("new: -c needs a directory"))?,
                );
            }
            _ if name.is_none() => name = Some(arg),
            other => {
                return Err(bad_input(&format!("new: unexpected argument {other:?}")));
            }
        }
    }
    if window {
        if let Some(named) = name {
            return Err(bad_input(&format!(
                "new -a opens a window on a session the window itself creates, so it cannot also \
                 be named {named:?} here — run `sprag new {named}` then `sprag attach {named}`"
            )));
        }
        return new_window_on_its_own_session();
    }
    let mut conn = connect()?;
    let mut args = match &name {
        Some(name) => json!({ "name": name }),
        None => json!({}),
    };
    if let Some(dir) = &cwd {
        args.as_object_mut()
            .expect("json! built an object")
            .insert(sprag_host::wire::SPAWN_CWD_KEY.to_owned(), json!(dir));
    }
    let answer = invoke_action(
        &mut conn,
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
        // ⚠ THIS ARM IS UNREACHABLE AGAINST A DAEMON OF THIS BUILD and is kept as the pre-R325
        // degradation only. The two refusals it lists — the name is taken, or it breaks the grammar
        // an address has to satisfy (`sprag_terminal::SessionName`) — are ONE fact the daemon now
        // states (`SessionError` / `SessionNameError`), and `invoke_action` prints it before
        // anything here runs. It survives because a daemon older than PINION-PR82 answers a bare
        // refusal under `Other`, which is exactly what this reads.
        Err(error) if error.kind() == io::ErrorKind::Other => {
            let named = name.as_deref().unwrap_or_default();
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "a session named {named:?} already exists, or that name is blank, over 80 \
                     bytes, or contains a control character"
                ),
            ))
        }
        Err(error) => Err(error),
    }
}

/// Launch a window that makes a session of its own — the body of `new -a`.
///
/// Spawned exactly as [`attach`] spawns one, and through the same [`own_session`], for the same
/// reason: a window must outlive the shell that opened it, so a hangup here cannot reach it.
/// The one difference from `attach` is the env, and it is the whole point — [`GUI_NEW_ENV`]
/// instead of [`GUI_SESSION_ENV`].
///
/// It returns as soon as the DAEMON has witnessed the window ([`await_window`]), not when the spawn
/// succeeded, so a window that dies on startup — no display, a broken binary — is this command's
/// failure rather than a silent exit 0 and a prompt that looks fine. That is `attach --no-wait`'s
/// discipline, and it is the right default here because `new` has always returned to the shell.
fn new_window_on_its_own_session() -> io::Result<()> {
    let sock = socket_path(HOST_SOCKET);
    // The pre-flight is the connection itself: a window is about to be launched against THIS
    // daemon, and a daemon that cannot be reached is worth saying so before a process is spawned to
    // find out. `attach` checks the session too; there is no session to check here.
    let mut conn = connect()?;
    let mut command = Command::new(client_bin(GUI_BIN_ENV, "sprag-gui"));
    command
        .env(GUI_NEW_ENV, "1")
        .env("SPRAG_GUI_HOST_SOCK", &sock)
        // ⚠ REMOVED, not merely left unset: this process's own environment is inherited, and a
        // person running `sprag new -a` from a shell that `sprag attach` exported into would hand
        // the window BOTH words. The client reads the session name first, so the launch would
        // quietly attach — the exact opposite of what was typed.
        .env_remove(GUI_SESSION_ENV);
    let mut child = own_session(&mut command).spawn().map_err(|error| {
        io::Error::new(error.kind(), format!("could not launch sprag-gui: {error}"))
    })?;
    await_window(&mut conn, &mut child)
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
    let answer = invoke_action(
        &mut conn,
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
        .env(GUI_SESSION_ENV, &name)
        .env("SPRAG_GUI_HOST_SOCK", &sock)
        // The complement of what `new -a` removes, and for the same reason: a shell that exported
        // the *new* word must not turn an explicit `attach NAME` into a create.
        .env_remove(GUI_NEW_ENV);
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
        let clients: Value = query_slot(conn, json!({ "path": mux_action_path(CLIENTS_SLOT) }))?;
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
/// `handle_parsed`), and its own answer is discarded — this asks only whether the scope resolved.
///
/// ## ⚠ THE SLOT MUST BE ONE WHOSE SUBJECT IS A SESSION, and since R327 that is a real choice
///
/// "Any scoped path answers the question" stopped being true when the daemon learned to serve a
/// read whose subject is the REGISTRY on a scope it cannot resolve (`sprag_host::registry_scene`) —
/// which it does because a `detach-on-destroy` policy must be able to re-read the session list at
/// the instant its own session dies. `sessions`, `tree`, `clients` and the rest of that half now
/// answer for a name no session carries, so a pre-flight probing one of them would report that
/// EVERY session exists and every scoped verb would sail past its own guard. Measured, not
/// supposed: moving this one line to `sessions` reddens four tests in `tests/cli.rs`, among them
/// *"attach to a missing session fails"*.
///
/// `windows` is on the right side of that split by its nature — a session's window list is a fact
/// about ONE session — and `rpc`'s `every_declared_read_is_measured_for_whether_it_needs_the_
/// readers_session` pins which half each address is in, by driving them.
/// # A scoped query carries TWO things that can be invalid, and this used to read them as one
///
/// The comment here said *"a scoped query carries nothing else that can be invalid"*, and the
/// stand-in daemon in `tests/cli.rs` refuted it by running: an unknown ADDRESS is
/// `UnknownIntrospectPath` under the SAME `INVALID_PARAMS` code a refused scope arrives under, so
/// against a daemon that does not serve `windows` this function answered `false` and
/// `sprag windows -t 0` reported **`no session named "0"`** about a session that was there. A
/// wrong answer that parses, from the pre-flight every scoped command runs — the exact failure
/// [`query_slot`] exists to prevent, in the one reader that could not use it.
///
/// So the code is the first half of the test and [`unknown_slot`] is the second. A refused SCOPE
/// is still read from the code rather than from a sentence, for the reason it always was: matching
/// wording would make this file depend on how another crate phrases itself.
fn session_exists(conn: &mut HostConn, name: &str) -> io::Result<bool> {
    let path = mux_action_path(WINDOWS_SLOT);
    match query_raw(conn, json!({ "session": name, "path": path.clone() })) {
        Ok(_) => Ok(true),
        Err(CallError::Fault(fault)) if fault.code == INVALID_PARAMS => {
            // The ADDRESS was refused, not the scope: this call learned nothing about the session
            // and must say so rather than answer for it.
            match unknown_slot(&path, &fault) {
                Some(skew) => Err(skew),
                // The daemon heard the request and refused the SCOPE — that IS the answer.
                None => Ok(false),
            }
        }
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
    client_beside(
        std::env::var_os(env_override),
        std::env::current_exe().ok().as_deref(),
        bin,
    )
}

/// [`client_bin`]'s DECISION, separated from the process and the environment it reads — the split
/// `sprag_host::host`'s `sprag_beside` and `mcp_beside` are written with, for their reason.
///
/// # ⚠⚠⚠⚠⚠ The sibling step is the whole of register item 463's construction half, and NOTHING
/// exercised it
///
/// *"The GUI a person launches is the daemon's own build"* rests entirely on the middle branch
/// here, and `current_exe` is process-global — so a test of the rule was a test nothing could
/// drive, and every CLI gate over `attach` sets the override env and returns at the FIRST branch.
/// The branch the claim depends on was reached by no test in this tree.
///
/// # ⚠⚠⚠ Why the last branch is a `PATH` name and not a refusal
///
/// It is `sprag_beside`'s ruling and not `mcp_beside`'s, and the two differ for a stated reason: an
/// MCP server's whole value is its VINTAGE, so a copy of unknown age is worse than none. A display
/// client's value is that a person gets their window — and in both layouts this product ships (a
/// cargo build tree, and `cargo install`'s one `bin`) the sibling is there, so this branch is a
/// deployment that copied SOME of the binaries. Refusing there would hand a person no window at
/// all rather than one to be doubted.
///
/// ⚠⚠ **And the doubt is now REPORTED rather than assumed away**: a client launched off `PATH`
/// states its build at `client/hello` like any other, so `sprag doctor` names it if it is not this
/// daemon's image ([`attached_build_report`]). Construction where it reaches, report where it
/// cannot — which is item 463's own ruling, applied to its own fallback.
fn client_beside(env_override: Option<OsString>, exe: Option<&Path>, bin: &str) -> PathBuf {
    if let Some(path) = env_override {
        return PathBuf::from(path);
    }
    if let Some(sibling) = exe
        .and_then(Path::parent)
        .map(|dir| dir.join(bin))
        .filter(|sibling| sibling.exists())
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
        Ok(answer) => {
            println!("{}", killed_sentence(&name, &answer, Ended::Session));
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

/// `kill-server [--purge]`: STOP the daemon, through the daemon's own shutdown edge.
///
/// By DEFAULT the durability snapshot is PRESERVED: stopping the daemon does not destroy the saved
/// workspace, so the next launch restores it (the cmux-durable model — your workspace persists).
/// `--purge` additionally DELETES the snapshot and every pane's saved scrollback: the explicit
/// "start fresh", the one way to destroy the saved workspace.
///
/// # ⚠⚠⚠ IT USED TO KILL EVERY SESSION, AND THAT BROKE THE PROMISE ABOVE
///
/// The old shape *"reuses `kill_session` over one connection — the last kill is what stops the
/// server"* was measured wrong on the owner's own daemon (2026-08-16). Sessions die ONE AT A TIME
/// while the durability saver keeps running on its five-second tick, so **the snapshot converges
/// toward empty as the kill proceeds**. What came back after that run was sessions `2`–`6`; session
/// `1`, the first one killed, was gone from the file — its panes, its layout, its agent. The doc
/// said the workspace persists and the implementation was deleting it a session at a time.
///
/// The cause is that `kill_session` MEANS destroy, and the daemon cannot tell *"stop the server"*
/// from *"destroy this session"* when the only word it is sent is the second one.
///
/// # ⚠⚠⚠ AND IT COULD NOT BE REACHED AT ALL WHEN IT WAS MOST NEEDED
///
/// A wire skew refuses every request at `client/hello`, and `kill-server` was a request — so the
/// daemon's own refusal advised a command that the same refusal blocked. **The remedy for a skew was
/// behind the skew.** Measured the same evening: the CLI could not stop a daemon one protocol
/// version behind it, and the way through was a hand-written script speaking the daemon's older wire.
///
/// # What it does instead, and why this is the product's door rather than a way around it
///
/// The daemon already HAS exactly one shutdown routine, and SIGTERM is its door: `install_shutdown`
/// cancels and joins in-flight plugin runs, and the last-pane edge reaches it by raising SIGTERM
/// into itself (`spawn_reaper`'s `on_empty`). So this asks for the same thing the daemon asks of
/// itself.
///
/// The pid comes from the SOCKET's peer credentials, which is what makes both fixes one fix:
///
/// * it is the pid of whoever is actually serving that socket — never a stale pidfile, never a name
///   match on a process table that may hold a second daemon;
/// * and **reading it involves no protocol at all**, so a skewed daemon is stopped exactly as
///   easily as a matched one. The version handshake exists to stop a skewed pair MISREADING each
///   other's shapes; a signal has no shape to misread.
///
/// ⚠ The residue, stated: this is unix-only, and a daemon that ignores SIGTERM is reported rather
/// than escalated to `SIGKILL`. Escalating is a decision about somebody's running work, so it stays
/// theirs.
fn kill_server(args: Vec<String>) -> io::Result<()> {
    let purge = args.iter().any(|a| a == "--purge");
    if let Some(other) = args.iter().find(|a| *a != "--purge") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kill-server: unexpected argument {other:?} (only --purge is accepted)"),
        ));
    }
    let endpoint = HostEndpoint::for_opts(HOST_SOCKET);
    let pid = serving_pid(endpoint.path())?;
    a_daemon(pid)?;
    stop_and_wait(pid)?;
    if purge {
        clear_snapshot();
        println!("server stopped (workspace purged)");
    } else {
        println!("server stopped");
    }
    Ok(())
}

/// How long to wait for a signalled daemon to go, and how often to look.
///
/// It has in-flight plugin runs to cancel and join before it exits, so this is a bound on a
/// shutdown that is DOING something rather than a guess at scheduling latency.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
const STOP_POLL: Duration = Duration::from_millis(50);

/// The pid of the process SERVING `path`, read from a fresh connection's peer credentials.
///
/// A connection and nothing else: no handshake, no request, no protocol — which is the whole reason
/// [`kill_server`] can reach a daemon whose wire this build does not speak.
///
/// # Errors
///
/// [`io::ErrorKind::NotFound`] when nothing is listening (the daemon is already gone, which
/// `kill-server`'s caller wants said plainly), else the `getsockopt` failure.
#[cfg(unix)]
fn serving_pid(path: &std::path::Path) -> io::Result<libc::pid_t> {
    use std::os::fd::AsRawFd;

    let sock = std::os::unix::net::UnixStream::connect(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no server running at {}",
                HostEndpoint::for_opts(HOST_SOCKET)
            ),
        )
    })?;
    peer_pid(sock.as_raw_fd())
}

/// The peer's pid on a connected unix socket. Linux spells it `SO_PEERCRED` over a `ucred`; macOS
/// spells it `LOCAL_PEERPID` over a bare `pid_t`. Both are the KERNEL's answer about the process on
/// the other end, which is what makes it unforgeable by anything but that process.
#[cfg(target_os = "linux")]
fn peer_pid(fd: std::os::fd::RawFd) -> io::Result<libc::pid_t> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = u32::try_from(size_of::<libc::ucred>()).expect("a ucred fits in a socklen");
    // SAFETY: `cred` is a live, correctly-sized `ucred` and `len` names its size; `getsockopt`
    // writes at most that many bytes into it and updates `len`. The fd is owned by the caller's
    // live `UnixStream`.
    let got = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut cred).cast(),
            &raw mut len,
        )
    };
    if got != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.pid)
}

/// See [`peer_pid`] on Linux — the same question, the other spelling.
#[cfg(target_os = "macos")]
fn peer_pid(fd: std::os::fd::RawFd) -> io::Result<libc::pid_t> {
    let mut pid: libc::pid_t = 0;
    let mut len = u32::try_from(size_of::<libc::pid_t>()).expect("a pid fits in a socklen");
    // SAFETY: as the Linux arm — a live, correctly-sized out-parameter and its length, over an fd
    // the caller's `UnixStream` owns.
    let got = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            std::ptr::from_mut(&mut pid).cast(),
            &raw mut len,
        )
    };
    if got != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

/// Refuse to signal anything that is not a sprag DAEMON — the guard between *"stop the server"* and
/// *"terminate whatever process happens to be serving this socket"*.
///
/// # ⚠⚠⚠ It exists because the suite caught the version without it, in one run
///
/// A host does not have to be a daemon. `sprag-peer`'s stand-in serves a socket from the TEST
/// process, and the first build of [`kill_server`] read that peer's pid and SIGTERMed it — killing
/// the harness (`process didn't exit successfully … signal: 15`). Every embedded host is that shape:
/// the socket names a SERVER, and the server may be a small part of something much larger that
/// nobody asked to end.
///
/// So the pid is checked against what it is RUNNING before it is signalled — [`exe_of`], which is
/// the one place either kernel's spelling of that question lives.
///
/// # ⚠⚠⚠⚠⚠ THE GUARD WAS VACUOUS ON macOS AND THE SUITE SAID SO IN THE ONLY WAY IT COULD
///
/// This used to read `/proc/<pid>/exe` on Linux and, on every other unix, narrow to *"the pid is
/// not THIS process"*. That fallback cannot ever be true of a real invocation: `kill-server` runs
/// inside the `sprag` CLI's own process and the server is, by construction, a different one — so on
/// macOS the guard let EVERY pid through, which is the whole hazard it was built for.
///
/// It was measured on 2026-08-20 as the third of item 487's macOS reds, and the shape of the
/// evidence is worth keeping: `sprag-host`'s `cli` target reported
/// `process didn't exit successfully … (signal: 15, SIGTERM)` with **no failing test anywhere in
/// the log**, because the harness serving the socket was the thing that got signalled. A suite that
/// is killed cannot report; the only witness was the exit status.
///
/// ⚠⚠ So the platform seam is now [`exe_of`] alone and the contract above it is UNCONDITIONAL —
/// register item 487's own lesson, which `0b83e8f` learned one file over: a kernel contract names
/// the platform it is true on, and the code above that name must not have to.
#[cfg(unix)]
fn a_daemon(pid: libc::pid_t) -> io::Result<()> {
    let exe = exe_of(pid)?;
    if runs_the_daemon(&exe) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "the process serving that socket (pid {pid}) is {}, not a `{}` daemon. \
             `kill-server` stops a daemon; it will not terminate a process that merely serves a \
             host socket, because that process may be doing something else nobody asked to end",
            exe.display(),
            sprag_rpc::DAEMON_BIN_NAME,
        ),
    ))
}

/// The one sentence for a pid whose executable this build cannot read — it is GONE, as far as any
/// decision here is concerned, and a guard that cannot tell must never wave the signal through.
#[cfg(unix)]
fn unreadable(pid: libc::pid_t) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("the process serving that socket (pid {pid}) is gone"),
    )
}

/// **WHAT `pid` IS RUNNING, AS THE KERNEL ANSWERS IT** — the fact [`a_daemon`] decides on, and the
/// only line in this file that has to know which unix this is.
///
/// Linux publishes it as a symlink nothing but the process itself can change. ⚠ The value can carry
/// a ` (deleted)` suffix once the binary has been replaced underneath the running process, which is
/// [`runs_the_daemon`]'s business rather than this function's — see its doc, because that case is
/// the promotion rather than an edge.
///
/// # Errors
///
/// When the process is gone, or when this kernel will not say. **Never an `Ok` that means *"I could
/// not tell"***: that is what made the old macOS arm let every pid through.
#[cfg(target_os = "linux")]
fn exe_of(pid: libc::pid_t) -> io::Result<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| unreadable(pid))
}

/// See [`exe_of`] on Linux — the same question, the other kernel's spelling.
///
/// Darwin has no `/proc`; `proc_pidpath` is its answer, and it is the reason item 487(c) is a fix
/// rather than a residue. ⚠ It reports the path the process was EXECUTED from and does not mark a
/// replaced binary the way Linux does, so the suffix [`runs_the_daemon`] strips simply never
/// appears here — which costs nothing, because stripping a suffix that is absent is a no-op.
///
/// # Errors
///
/// When `proc_pidpath` writes nothing — the process is gone, or this caller may not ask about it.
#[cfg(target_os = "macos")]
fn exe_of(pid: libc::pid_t) -> io::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let room = u32::try_from(path.len()).expect("a path buffer fits in a u32");
    // SAFETY: `path` is a live buffer of exactly `room` bytes and `proc_pidpath` writes at most
    // that many into it, returning how many. The pid is a plain integer the kernel validates.
    let wrote = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), room) };
    let wrote = usize::try_from(wrote).unwrap_or(0);
    if wrote == 0 {
        return Err(unreadable(pid));
    }
    Ok(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
        &path[..wrote],
    )))
}

/// Whether `exe` — a path read from `/proc/<pid>/exe` — is the sprag daemon.
///
/// # ⚠⚠⚠⚠ ` (deleted)`, AND WHY IT IS THE CASE THAT MATTERS MOST
///
/// Linux appends ` (deleted)` to that symlink's target once the binary has been REPLACED underneath
/// the running process. That is not an edge case here — **it is the promotion**: the whole reason to
/// stop a daemon is that a new build exists, and building is what deletes the inode it is running
/// from. So the first version of this check refused every daemon it was written to stop, and said so
/// in a sentence naming a path ending in `(deleted)`.
///
/// Measured on the owner's own daemon, by running the fixed `kill-server` at the one moment it was
/// for. ⚠ A guard that is wrong exactly when it is needed is worse than no guard: it reads as a
/// product that does not work rather than as a rule doing its job.
fn runs_the_daemon(exe: &std::path::Path) -> bool {
    /// What the kernel appends once the running binary's directory entry is gone.
    const UNLINKED: &str = " (deleted)";

    exe.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.strip_suffix(UNLINKED).unwrap_or(name))
        == Some(sprag_rpc::DAEMON_BIN_NAME)
}

/// Ask `pid` to shut down and WAIT until the socket stops SERVING — the daemon's own edge, and then
/// the proof.
///
/// Waiting is the substance rather than politeness: returning while the daemon is still cancelling
/// runs would let the very next command (a promotion's relaunch, a script's next line) race a socket
/// that is about to be unlinked, which reads as *"no server running"* on a daemon only halfway out.
///
/// # ⚠⚠⚠ The proof is the SOCKET, and `kill(pid, 0)` was measured wrong for it
///
/// The first version polled the process instead, and the suite hung on it for the full deadline
/// twice. **`kill(pid, 0)` succeeds for a ZOMBIE**: the daemon had exited, but it was a child of the
/// test harness, which does not reap until its own `Drop` — so *"the process still answers"* stayed
/// true long after the server was gone. In production the CLI is not the daemon's parent and the pid
/// disappears promptly, which is exactly the kind of difference a harness exposes and a live run
/// hides.
///
/// The socket is also the better QUESTION. This verb is about a SERVER, and what its caller needs to
/// know is whether the next command will find one — not whether an entry still exists in a process
/// table. A refused connect answers that directly, and it is the same reading the daemon's own
/// *"no server running at …"* is built on.
#[cfg(unix)]
fn stop_and_wait(pid: libc::pid_t) -> io::Result<()> {
    // SAFETY: a plain signal send; no memory is shared with the kernel here.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        let failed = io::Error::last_os_error();
        // ESRCH is the daemon having gone between the connect and the signal — the outcome asked
        // for, reached by somebody else, which is not a failure of this command.
        if failed.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(failed);
    }
    let path = HostEndpoint::for_opts(HOST_SOCKET).path().to_owned();
    let began = Instant::now();
    while began.elapsed() < STOP_DEADLINE {
        if std::os::unix::net::UnixStream::connect(&path).is_err() {
            return Ok(());
        }
        std::thread::sleep(STOP_POLL);
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "the server (pid {pid}) was asked to stop and was still serving {}s later. It may be \
             joining a plugin run that will not end. Nothing here escalates to SIGKILL — that is a \
             decision about the work inside it, so it stays yours",
            STOP_DEADLINE.as_secs(),
        ),
    ))
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

/// Issue one `kill_session {name}` — the shared call behind both kill commands — and hand back the
/// daemon's answer, which carries how far the kill CASCADED ([`ENDED_KEY`]).
///
/// `kill-server` discards it (it is killing every session by construction, so "the server went too"
/// is not news); `kill-session` renders it.
fn kill_one(conn: &mut HostConn, name: &str) -> io::Result<Value> {
    invoke_action(
        conn,
        json!({ "path": mux_action_path(KILL_SESSION_ACTION), "args": { "name": name } }),
    )
}

/// Split a window subcommand's args into its `-t SESSION` target and any trailing positionals. A
/// window lives IN a session, and the daemon holds several — so, like tmux's window/pane commands,
/// these take `-t`.
///
/// The target comes back as an OPTION and the "one is required" refusal lives in [`require_target`],
/// which is where a connection exists: a caller running inside a pane needs no `-t`, and only the
/// daemon can say which session that pane is in. Deciding it here would mean deciding it before
/// there is anybody to ask.
fn target_and_rest(args: Vec<String>, command: &str) -> io::Result<(Option<String>, Vec<String>)> {
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
    Ok((session, rest))
}

/// The request params addressing `path`, carrying the out-of-band `session` scope — the ONE place a
/// scoped request is shaped, so every command spells the scope the same way the GUI does
/// ([`sprag_host::wire::SESSION_PARAM`]).
///
/// # Which session an unnamed scope means
///
/// The caller's `-t` first, then the session this process is RUNNING IN ([`Here`]), and only then
/// nothing — which lets the daemon pick its default.
///
/// The middle term is the one that changed. It used to go straight from "the caller named none" to
/// "send no key", under a comment saying that inventing a name here would freeze today's answer
/// into the wire. That reasoning holds for a name this CLI GUESSES and not for the one it is
/// standing in: `$SPRAG_PANE` is the daemon's own statement about this process, and reading it back
/// is the opposite of a guess. What the old default actually froze was the assumption that the
/// caller is a shell somewhere outside the workspace — see [`Here`] for what it cost the callers
/// who are not.
fn scoped_params(session: Option<&str>, path: String) -> Value {
    match effective_scope(session) {
        Some(name) => json!({ "session": name, "path": path }),
        None => json!({ "path": path }),
    }
}

/// [`scoped_params`] plus the action's `args` — for the verbs whose arguments are BUILT rather than
/// written as a `json!` literal beside the path.
///
/// Most acting verbs spell the whole request in one `json!`, which is right when the keys are known
/// at the call site. `orchestrate` and `cancel-run` are not: their arguments come from the daemon's
/// own published grammar, so the object arrives as a value and the scope has to be put around it.
fn scoped_call(session: Option<&str>, path: String, args: Value) -> Value {
    let mut params = scoped_params(session, path);
    params["args"] = args;
    params
}

/// The scope ALONE, for the two methods that read no path — `scene/revision` and `scene/waitFor`.
/// Kept beside [`scoped_params`] so the one way a request names its session is spelled once.
fn scoped_only(session: Option<&str>) -> Value {
    match effective_scope(session) {
        Some(name) => json!({ "session": name }),
        None => json!({}),
    }
}

/// The session a request is about: what the caller named, else where this process is running.
///
/// Spelled once because it is the whole behaviour — a second copy in the invoke path is how the
/// read and the write of one command would come to disagree about which session they meant.
fn effective_scope(session: Option<&str>) -> Option<&str> {
    session.or_else(|| here().map(|here| here.session.as_str()))
}

/// [`scoped_params`] plus an action's `args` — the invoke shape, kept beside the query shape so the
/// two cannot drift.
fn scoped_invoke(session: Option<&str>, path: String, args: Value) -> Value {
    let mut params = scoped_params(session, path);
    params["args"] = args;
    params
}

/// [`scoped_invoke`] for a resolved pane, carrying the window it lives in.
///
/// Every pane-addressed INVOKE goes through here, not only the ones that need it. Measured at
/// `e7be5eb`: `rename_pane` / `swap_pane` / `zoom_pane` resolve REGISTRY-wide at the daemon and
/// `select_pane` / `close` / `split` resolve against the SCOPE's window — so a client that tried to
/// remember which is which would be keeping a second copy of the daemon's rule, free to drift.
/// Sending the window always is correct for both: the registry-wide actions do not consult it.
fn site_invoke(session: Option<&str>, site: &PaneSite, path: String, args: Value) -> Value {
    let mut params = site_params(session, site, path);
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

/// Name the scope in an error message: the session the caller asked for, the one it is RUNNING in,
/// or the honest stand-in for neither.
fn scope_name(session: Option<&str>) -> &str {
    session
        .or_else(|| here().map(|here| here.session.as_str()))
        .unwrap_or("the default session")
}

/// Where this process IS: the pane the daemon gave it at birth, and the session that holds that
/// pane now.
///
/// # The wrong answer this exists to remove
///
/// The daemon tells every pane's child which pane it is ([`sprag_host::PANE_ENV_VAR`], tmux's
/// `$TMUX_PANE`) and nothing on this side ever asked what that meant. So an unscoped command run
/// INSIDE a pane went to *the daemon's default session*, which is a different session from the
/// caller's as soon as anybody makes a second one. Measured by running, from a pane of session
/// `work` on a daemon whose default was `0`:
///
/// ```text
/// sprag panes           -> lists session 0's panes, not work's
/// sprag layout          -> draws session 0
/// sprag split-window    -> the new pane appears in session 0, and the command reports success
/// ```
///
/// A person only ever sees this as their command acting on somebody else's session, silently. It is
/// worse for an AGENT, which is exactly the caller that runs inside a pane and has no other way to
/// know where it is.
///
/// # Why the caller's OWN pane, and not the focused one
///
/// The alternative — resolving "here" to whatever pane a person is looking at — is what the rival
/// does (`herdr`'s `--current` sends no pane and its daemon falls back to
/// `state.active` + `focused_pane_id()`, `src/app/api/panes.rs`), and it is wrong for the case that
/// matters most: two agents working in two panes both get the answer for whichever pane a human
/// happens to be watching, and neither can address itself. `$SPRAG_PANE` is per-CALLER, so it
/// cannot confuse them. It is also what tmux means by the current pane for a command run inside one.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Here {
    /// The pane this process was born in — [`sprag_host::PANE_ENV_VAR`], as the daemon set it.
    pane: u64,
    /// The session holding that pane NOW, read from the registry rather than remembered: a pane's
    /// session is the daemon's fact, and the environment carries no name to go stale.
    session: String,
    /// The WINDOW holding that pane now, on the line above's terms exactly — register item 754.
    ///
    /// # ⛔⛔⛔⛔⛔ The half this type argued for and did not carry
    ///
    /// The section above — *why the caller's OWN pane, and not the focused one* — is an argument
    /// about a FOCUS, and it was only ever finished for the session. A request that names no window
    /// acts in the session's CURRENT one, so an unscoped command run inside a pane went on landing
    /// wherever a person was looking: *two agents working in two panes both get the answer for
    /// whichever one a human happens to be watching*, which is the sentence above, one level down
    /// and still true.
    ///
    /// Measured 2026-08-29: a `split-window` from a process standing in window `sprag`, with the
    /// session's current window `sce`, put the pane in `sce`. `$SPRAG_PANE` is per-CALLER and the
    /// window holding it is the daemon's own fact, so reading it back is the opposite of a guess —
    /// and it is the only answer here that is the same every time it is asked.
    ///
    /// [`None`] when the daemon's tree does not place the pane, which is [`session`](Self::session)
    /// answering at all: they are read off ONE tree at one instant, so they cannot describe two
    /// places.
    window: Option<String>,
}

/// This process's [`Here`], or [`None`] when it is not running in a pane THIS daemon holds.
///
/// Resolved once by [`learn_where_we_are`] as a condition of connecting, and read from here after —
/// so the params builders below can consult it without every one of forty call sites growing a
/// connection argument. A CLI process makes one connection and exits; the value cannot change
/// underneath it, which is what makes a cache honest rather than a shortcut.
fn here() -> Option<&'static Here> {
    HERE.get().and_then(Option::as_ref)
}

/// [`here`]'s cell. Filled exactly once, by [`learn_where_we_are`].
static HERE: std::sync::OnceLock<Option<Here>> = std::sync::OnceLock::new();

/// Ask the daemon where `$SPRAG_PANE` is, and remember it for this process.
///
/// # It is silent about every failure, deliberately
///
/// This runs on the way to every command, and none of its failures is the caller's business: no
/// `$SPRAG_PANE` means the caller is a shell rather than a pane, a pane id this daemon does not
/// hold means the variable outlived the daemon that set it (ids restart with a process), and a
/// daemon too old to serve the tree cannot answer at all. Each of those is the same situation for
/// the caller — *nobody said which session, so the daemon's default is the one* — which is the
/// behaviour that was already there. That is why it reads through [`query_raw`] rather than
/// [`query_slot`]: the skew SENTENCE is right for a command a person typed and wrong for a question
/// this asked on its own.
fn learn_where_we_are(conn: &mut HostConn) {
    if HERE.get().is_some() {
        return;
    }
    let _ = HERE.set(where_we_are(conn));
}

/// [`learn_where_we_are`]'s body, split out so the resolution is a function of the daemon's answer.
fn where_we_are(conn: &mut HostConn) -> Option<Here> {
    let pane = our_pane()?;
    let answer = query_raw(conn, json!({ "path": mux_action_path(TREE_SLOT) })).ok()?;
    let tree: Vec<sprag_terminal::TreeSession> = serde_json::from_value(answer).ok()?;
    // The lookup itself belongs to neither client: `sprag-mcp` asks the same question of the same
    // slot so an agent's tools answer about its own session, and two copies of "which session holds
    // this pane" is a torn answer waiting to happen.
    let session = sprag_host::wire::session_holding(&tree, PaneId(pane))?.to_owned();
    // ⚠⚠ OFF THE SAME TREE, AT THE SAME INSTANT — register item 754. A second read would be a
    // second answer, and this type's whole contract is that the two levels describe ONE place.
    let window = sprag_host::wire::window_holding(&tree, PaneId(pane)).map(str::to_owned);
    Some(Here {
        pane,
        session,
        window,
    })
}

/// The WINDOW this process is standing in, for a request scoped to the session it is standing in —
/// register item 754.
///
/// # ⚠⚠⚠ Why the scope guard, which is [`resolve_optional_pane`]'s and not a new rule
///
/// With an explicit `-t elsewhere` the ambient pane is not in the session being addressed at all,
/// so its window names nothing there — and substituting it would turn a scoped command into a
/// wrong answer of exactly the kind [`Here`] exists to remove. The guard is spelled the same way
/// one function over, because it is the same condition about the same fact.
///
/// [`None`] means *do not narrow*, which is what every caller sent before this existed: the
/// daemon's current window, unchanged for a caller that is not standing in a pane of this session.
fn here_window(session: Option<&str>) -> Option<&'static str> {
    here()
        .filter(|here| effective_scope(session) == Some(here.session.as_str()))?
        .window
        .as_deref()
}

/// The pane this process was born in — but only when the environment ALSO carries the address that
/// pane was published beside, and that address is the daemon this command is talking to.
///
/// # A pane id means nothing without the daemon it belongs to
///
/// Pane ids are per-daemon and start at zero, so a box running two sprag terminals has two pane
/// `1`s. An id read on its own therefore names a real, plausible pane of whichever daemon is being
/// asked — and the session it resolves to would be wrong in the one way nobody can see. That is why
/// [`sprag_host::pane_env_source`] publishes the id and the socket TOGETHER at each pane's birth:
/// they are one fact, and half of it is not a smaller truth.
///
/// `sprag-mcp` has had this guard since it started reading the variable at all (`own_pane`, which
/// compares the two as PATHS rather than as text). This is the same rule at the other client — a
/// rule kept by one of two doors is the shape this project keeps finding defects in.
fn our_pane() -> Option<u64> {
    let published = std::env::var_os(HOST_SOCKET.path_env)?;
    // As a PATH, not as text: two spellings of one socket are one daemon.
    (std::path::Path::new(&published) == HostEndpoint::for_opts(HOST_SOCKET).path())
        .then_some(())?;
    std::env::var(sprag_host::PANE_ENV_VAR)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// [`scoped_params`] narrowed to the window the CALLER IS STANDING IN — what every unnarrowed read
/// about *this window* must send (register item 759).
///
/// # ⛔⛔⛔⛔⛔ The lie this removes
///
/// A read that names no window is answered about the session's CURRENT one — *whichever a person
/// is looking at* — and three readers here describe their answer as the caller's own window
/// anyway. [`find`]'s comment says *every pane of the caller's own window*; `panes`' help says
/// *the current window's panes* and a caller standing in a pane reads that as theirs.
///
/// Measured 2026-08-30 on this repository's own daemon: standing in window `sprag` with the
/// session's current window `wz`, `sprag panes -t loop` answered `outer-wz` and `inner-wz` —
/// another repository's live loop.
///
/// ⚠⚠ **A CALLER STANDING IN NO PANE OF THIS SESSION NARROWS NOTHING**, which is byte-identical to
/// the request these have always made: [`here_window`] answers [`None`] there, and the daemon's
/// current window is the only answer a shell outside the workspace could ever mean.
fn here_params(session: Option<&str>, path: String) -> Value {
    windowed_params(session, path, here_window(session))
}

/// **THE WINDOW A READ VERB WAS POINTED AT** — the pane it was handed, resolved to the window
/// holding it, or [`None`] for *wherever this verb looks by default*. Register item 782.
///
/// # ⛔⛔⛔⛔⛔ What was wrong, and the sentence that already forbade it
///
/// [`windowed_params`]'s own doc says it is *"the one place a CLI request learns which window to
/// act in, **so a verb added later cannot forget it and quietly become window-local again**"*.
/// `layout` and `panes` are exactly the verbs that forgot: both refused every positional argument
/// (*"only -t SESSION is accepted"*), so neither could be asked about a window the caller is not
/// standing in. That sentence was prose, and nothing measured it — this workspace's rule 10.
///
/// **What it cost, measured 2026-08-31**: item 772's question is the owner's — *which windows are
/// split which way* — and answering it needs the four windows COMPARED. With no way to name one,
/// the only route was `select-window` to each in turn, which is the act item 697 paid to forbid
/// (it takes the owner's view away and they cannot put it back). So a live question was
/// unanswerable except by breaking a rule.
///
/// # ⚠⚠ The daemon could always do this, and the MCP mouth already asks it
///
/// `mcp__sprag__pane_layout` takes a `pane` and sends [`windowed_params`]' equivalent — *"name a
/// pane (by NAME) in ANOTHER window and it draws THAT window instead"*. The repair is therefore one
/// SURFACE, not a wire or a daemon change, and it is the same size as the one register item 686
/// made for `agent`, `processes` and `capture-pane`.
///
/// ⚠ **AND THE DIRECTION IS NOT THE ONE THE LAST SUCH ITEM HAD.** Item 687 was the MCP mouth
/// lagging the CLI; this is the CLI lagging the MCP. A reader who remembers the shape and not the
/// measurement gets it backwards, so which mouth is ahead is re-measured per item.
///
/// # ⚠ [`None`] from a resolved pane is not a failure
///
/// [`PaneSite::window`] answers [`None`] for a pane of the caller's OWN window, deliberately — see
/// its doc. A verb reading that keeps its own default, which for `panes` is [`here_params`] (the
/// caller's window) and for `layout` is the scope's current one; the two differ, and register item
/// 759 is why `panes` must not quietly become the other.
fn read_target_window(
    conn: &mut HostConn,
    session: Option<&str>,
    named: Option<&str>,
    command: &str,
) -> io::Result<Option<String>> {
    let Some(raw) = named else {
        return Ok(None);
    };
    Ok(resolve_pane(conn, session, raw, command)?.window)
}

/// The one PANE a read verb may be handed — refused **before a socket is opened** when there is
/// more than one. Register item 782.
///
/// ⚠⚠ Separate from [`read_target_window`] for exactly that reason, and it is a property an
/// existing gate had already pinned: *"an argument this verb does not take is refused locally,
/// naming what it does take."* Resolving a pane needs the daemon; counting the arguments does not,
/// and folding the two together would have made a typo cost a round trip to learn it was a typo.
fn one_pane_at_most<'a>(rest: &'a [String], command: &str) -> io::Result<Option<&'a str>> {
    if let Some(extra) = rest.get(1) {
        return Err(bad_input(&format!(
            "{command}: unexpected argument {extra:?} (one PANE and -t SESSION are all it takes)"
        )));
    }
    Ok(rest.first().map(String::as_str))
}

/// The ids of the panes THE CALLER'S OWN window holds — the one read behind every pane-id check and
/// the `panes` listing, so a client and the daemon cannot disagree on which panes are addressable.
///
/// ⚠ Its window is [`here_params`]'s; see there for the answer this used to give and for the
/// callers whose own prose it makes true. A pane of ANOTHER window is still reachable — by id or by
/// name — through [`resolve_pane`]'s session-wide fall-through, which is register item 686's path
/// and is what keeps this narrowing from being a re-narrowing.
fn pane_ids(conn: &mut HostConn, session: Option<&str>) -> io::Result<Vec<u64>> {
    let listed: Value = query_slot(conn, here_params(session, mux_action_path(PANES_SLOT)))?;
    Ok(listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_u64())
        .collect())
}

/// A pane this CLI has RESOLVED — which pane, and which window of the scoped session holds it.
///
/// # Why a type, and why it has no public constructor
///
/// Before R312 every pane verb parsed its own argument and pre-flighted against the scoped
/// session's CURRENT WINDOW (a `require_pane` this round deleted, because a helper that can only
/// see one window is the defect and keeping it would let a verb grow back into it). That produced two measured defects at
/// `e7be5eb`, both of them on the same daemon at the same instant:
///
/// ```text
/// sprag zoom-pane 1     -> pane 1 fills its window          SUCCEEDS
/// sprag rename-pane 1 z -> pane 1 is now "z"                SUCCEEDS
/// sprag capture-pane 1  -> no pane 1 in 0 (panes: [0])      REFUSES
/// sprag select-pane 1   -> no pane 1 in the current window  REFUSES
/// ```
///
/// — because the succeeding verbs are REGISTRY-WIDE mux actions and the refusing ones went through
/// a window-local pre-flight. And no verb took a pane's NAME at all, in SIX different sentences.
///
/// A pane id is registry-unique and never reused, so it has no positional hazard and there was
/// never a reason for a read to see fewer panes than a write. Resolution is therefore SESSION-wide
/// for both spellings, and a `PaneSite` can only come out of [`resolve_pane`] — so a verb cannot
/// address a pane without having looked it up, which is the one way seven parses could not grow
/// back.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneSite {
    /// The pane's id — what every action and every pane-addressed path takes.
    id: u64,
    /// The window holding it, or [`None`] when nothing here can narrow one — which is a caller
    /// standing in no pane of this session, and for whom the scope's CURRENT window is the only
    /// answer there could be.
    ///
    /// `None` rather than always naming the window, deliberately: it keeps "narrowed" and "not
    /// narrowed" distinguishable, which is what the version-skew probe measures.
    ///
    /// ⚠⚠⚠⚠⚠ **IT USED TO MEAN *the scope's current window*, AND THAT STOPPED BEING TRUE** —
    /// register item 759, recorded here because this doc is what a reader trusts instead of
    /// checking (item 644). The fast path in [`resolve_pane`] answers a pane found in the listing
    /// [`pane_ids`] returns, and that listing is the CALLER's window now rather than the current
    /// one. `None` therefore had to stop standing in for it: the two are the same window only for
    /// a caller who is not in a pane, and that is exactly the case this now describes.
    window: Option<String>,
}

/// Resolve a pane argument — a NAME or an id — anywhere in the scoped SESSION.
///
/// The NAME half is the daemon's own grammar, read through
/// [`PaneAddress`] so the CLI and the agent surface split
/// digits from names by one rule, and refused through
/// [`unknown_pane_name_with`] so they refuse in
/// one sentence. The ID half stays a number because that is what `sprag panes` prints and what the
/// daemon's logs say.
/// ⛔⛔⛔⛔⛔ **THE PANE THIS COMMAND IS BEING TYPED IN**, when the daemon still holds it — register
/// item 871, and the reason a run launched from a shell can have an owner at all.
///
/// # ⚠⚠⚠⚠⚠ It repeats an identity, it does not assert one
///
/// `SPRAG_PANE` is written into a pane's environment BY THE HOST THAT SPAWNED IT. A process reading
/// it is not claiming to be somebody; it is saying which seat it was put in.
/// `PluginGrammar::OPENED_BY` already calls that key *"PROVENANCE and not authorisation"* and notes
/// this wire has no authentication at all — so nothing here is being trusted that was not already.
///
/// **And the forging objection was measured rather than argued.** `sprag-mcp` refuses to let an
/// agent set this key, on the grounds that it could then claim or cancel another pane's runs — the
/// right rule for a MEDIATED surface, and it is untouched. It does not carry over to this binary:
/// `send-keys` takes any pane id with no ownership check whatever, so a process that could forge
/// `SPRAG_PANE` can already type into the pane it would be impersonating. Refusing to record an
/// opener here protects nothing and costs every run its owner.
///
/// ⚠⚠ **AND A FORGER STILL CANNOT NAME A CONVERSATION.** The caller sends a PANE; the daemon reads
/// `agent_session` off that pane itself (`PluginsExternal::session_in`) and records the answer. So
/// the worst a wrong `SPRAG_PANE` can do is attribute a run to a real pane of this daemon — never
/// invent an owner, and never write a name of the caller's choosing.
///
/// # ⚠⚠⚠ A stale variable must NOT kill the run
///
/// `$SPRAG_PANE` outlives the daemon that set it — ids restart with the process — and the door
/// REFUSES a run whose opener names no pane it holds (`parse_opener`, deliberately, and its control
/// is a gate). Forwarding one blindly would turn a working `orchestrate` into a refusal for every
/// caller with a stale environment. So it is resolved first, through [`resolve_pane`] and not
/// [`pane_ids`], because an asker may be sitting one window over (register item 689) and a
/// current-window check would silently drop a good opener.
///
/// ⚠ Dropping it SAYS SO on stderr rather than quietly: a silent drop is how a whole product-wide
/// gap looked like a per-run coincidence for 190 runs, which is the state this item was opened in.
fn asking_pane(conn: &mut HostConn, session: Option<&str>) -> Option<u64> {
    let raw = std::env::var(sprag_host::PANE_ENV_VAR).ok()?;
    match resolve_pane(conn, session, &raw, "orchestrate") {
        Ok(site) => Some(site.id),
        Err(why) => {
            eprintln!(
                "orchestrate: ${} is {raw} and this daemon has no such pane ({why}), so this run \
                 will record no owner",
                sprag_host::PANE_ENV_VAR,
            );
            None
        }
    }
}

fn resolve_pane(
    conn: &mut HostConn,
    session: Option<&str>,
    raw: &str,
    command: &str,
) -> io::Result<PaneSite> {
    let here = pane_ids(conn, session)?;
    let address = PaneAddress::parse(raw);
    // A pane of THE CALLER'S OWN window resolves without reading any further — the ordinary case,
    // and one query, exactly as before this verb could reach past the window.
    //
    // ⛔⛔⛔⛔⛔ AND IT CARRIES THAT WINDOW — register item 759, found by a gate rather than
    // predicted. `window: None` means *the scope's current window* (see [`PaneSite::window`]), and
    // that was the same window `pane_ids` answered from right up until this item narrowed it. The
    // instant those two came apart, this fast path started saying *current* about a pane it had
    // found in the CALLER's: measured live at `sprag find --pane 1`, which resolved pane 1 out of
    // the caller's window and then died `NoExternalAtPath` asking the current one for it — while
    // the same pane reached BY NAME worked, because the name path goes session-wide and fills the
    // window in. **A listing narrowed at one end and addressed at the other is neither window's.**
    if let PaneAddress::Number(id) = &address
        && here.contains(id)
    {
        return Ok(PaneSite {
            id: *id,
            window: here_window(session).map(str::to_owned),
        });
    }
    let elsewhere = session_panes(conn, session)?;
    match &address {
        PaneAddress::Number(id) => elsewhere
            .iter()
            .find(|(_, pane, _)| pane == id)
            .map(|(window, id, _)| PaneSite {
                id: *id,
                window: Some(window.clone()),
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{command}: no pane {id} in {} (panes: {:?})",
                        scope_name(session),
                        elsewhere.iter().map(|(_, id, _)| *id).collect::<Vec<_>>(),
                    ),
                )
            }),
        PaneAddress::Name(name) => {
            let named: Vec<NamedPane> = elsewhere
                .iter()
                .filter_map(|(window, _, held)| {
                    Some(NamedPane::new(held.as_deref()?, window.clone()))
                })
                .collect();
            let mut bearers = elsewhere
                .iter()
                .filter(|(_, _, held)| held.as_deref() == Some(name.as_str()));
            let found = bearers.next().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{command}: {}",
                        unknown_pane_name_with(name, &named, PaneListing::SpragPanes),
                    ),
                )
            })?;
            if bearers.next().is_some() {
                // Unreachable through correct requests (the daemon holds names unique) and refused
                // rather than guessed for the agent surface's reason: a plausible answer against
                // the wrong pane is the failure a name exists to remove.
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{command}: {}",
                        ambiguous_pane_name(
                            name,
                            &named
                                .iter()
                                .filter(|pane| pane.name == *name)
                                .cloned()
                                .collect::<Vec<_>>(),
                        ),
                    ),
                ));
            }
            // ⛔⛔⛔⛔⛔ **THE WINDOW IS ALWAYS CARRIED, AND IT USED TO BE DROPPED FOR THE CURRENT
            // ONE** — register item 782, and the arm register item 759 did not reach.
            //
            // This read `(found.0 != current_window(…)).then(…)`, which encodes the meaning
            // `PaneSite::window`'s doc USED to have — *`None` is the scope's current window*. Item
            // 759 replaced that meaning (it is now *the CALLER's own window*, because the fast path
            // above answers from the caller's listing) and fixed the fast path and the doc; the
            // NAME path kept the old spelling, so the type's own doc and this line disagreed.
            //
            // **Measured live, 2026-08-31, against the loop daemon**: standing in window `sprag`
            // with `wz` current, `sprag panes inner-wz` listed `outer-sprag`/`inner-sprag` — the
            // CALLER's window — while `sprag panes 501` (the same pane, by id) answered `wz`. One
            // pane, two spellings, two windows.
            //
            // ⚠ Naming a window the request would have defaulted to costs nothing: the daemon
            // resolves the same window either way. What it buys is that `None` means ONE thing.
            Ok(PaneSite {
                id: found.1,
                window: Some(found.0.clone()),
            })
        }
    }
}

/// Resolve an OPTIONAL pane argument — absent means the verb's own default (the active pane), which
/// every caller of this already handled as [`None`].
fn resolve_optional_pane(
    conn: &mut HostConn,
    session: Option<&str>,
    raw: Option<&str>,
    command: &str,
) -> io::Result<Option<PaneSite>> {
    if let Some(raw) = raw {
        return resolve_pane(conn, session, raw, command).map(Some);
    }
    // NAMED NOTHING, and running inside a pane: that pane is the one it means. tmux's rule for a
    // command run inside a pane, and the reading `$SPRAG_PANE` has always deserved.
    //
    // Only when the scope IS this process's own session — with an explicit `-t elsewhere` the
    // ambient pane is not in the session being addressed at all, and substituting it would turn a
    // scoped command into a wrong answer of exactly the kind [`Here`] exists to remove.
    let Some(here) = here().filter(|here| effective_scope(session) == Some(here.session.as_str()))
    else {
        // Absent means the session's ACTIVE pane, which the DAEMON resolves — reading it back to
        // send it would race whoever moved it between the two calls.
        return Ok(None);
    };
    // Through the same resolution an argument gets, so the answer carries the window (and so a
    // stale id fails the same way rather than reaching the daemon as a target that is not there).
    resolve_pane(conn, session, &here.pane.to_string(), command).map(Some)
}

/// Every pane of the scoped SESSION as `(window, id, name)`, in the session's window order (R310).
///
/// Read window by window because the daemon has no session-wide pane slot and should not grow one
/// for this: the `panes` slot IS a window's listing (it is what a display client projects), and
/// R311 gave a REQUEST the ability to name its window rather than inventing a second listing free
/// to disagree with the first.
fn session_panes(
    conn: &mut HostConn,
    session: Option<&str>,
) -> io::Result<Vec<(String, u64, Option<String>)>> {
    let mut out = Vec::new();
    for window in window_names(conn, session)? {
        let listed: Value = query_slot(
            conn,
            windowed_params(session, mux_action_path(PANES_SLOT), Some(&window)),
        )?;
        for pane in listed.as_array().into_iter().flatten() {
            let Some(id) = pane["id"].as_u64() else {
                continue;
            };
            out.push((window.clone(), id, pane["name"].as_str().map(str::to_owned)));
        }
    }
    Ok(out)
}

/// The scoped session's window names, in the order the session arranges them.
fn window_names(conn: &mut HostConn, session: Option<&str>) -> io::Result<Vec<String>> {
    let listed: Value = query_slot(conn, scoped_params(session, mux_action_path(WINDOWS_SLOT)))?;
    Ok(listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|window| Some(window["name"].as_str()?.to_owned()))
        .collect())
}

// ⛔⛔⛔⛔⛔ `current_window` WAS HERE, AND ITS DOC WAS THE LAST COPY OF A RETIRED MEANING —
// register item 782. It read *"the scoped session's CURRENT window, which is what a `PaneSite` with
// no window means"*, and register item 759 had already stopped that being true: a `PaneSite` with no
// window is the CALLER's. Its one caller was the name road in `resolve_pane`, which used it to drop
// the window whenever a pane was in the current one — the defect 782 measured live — so removing
// that use left the function with no callers and clippy said so.
//
// ⚠ DELETED rather than kept for a future reader. A helper whose doc states a meaning the codebase
// has retired is not neutral: the next verb that wants *the current window* would reach for it and
// inherit the retired sentence with it, which is exactly how the name road came to disagree with
// its own type. The window a request means is `windowed_params`' to decide, and it is one place.
/// [`scoped_params`] carrying a WINDOW as well — the one place a CLI request learns which window to
/// act in, so a verb added later cannot forget it and quietly become window-local again.
fn windowed_params(session: Option<&str>, path: String, window: Option<&str>) -> Value {
    let mut params = scoped_params(session, path);
    if let (Value::Object(map), Some(window)) = (&mut params, window) {
        map.insert(
            sprag_rpc::WINDOW_PARAM.to_owned(),
            Value::String(window.to_owned()),
        );
    }
    params
}

/// [`windowed_params`] for a resolved pane — what every pane-addressed query and invoke sends.
fn site_params(session: Option<&str>, site: &PaneSite, path: String) -> Value {
    windowed_params(session, path, site.window.as_deref())
}

/// [`windowed_params`] plus an action's `args` — [`site_invoke`]'s form for a request that names a
/// window WITHOUT naming a pane, which is what a BIRTH is (register item 754).
///
/// Kept beside the other two so the one way a request names its window is spelled once: a birth
/// that built its own `{"window": …}` would be a second copy of that rule, free to drift from the
/// one every pane-addressed call already goes through.
fn windowed_invoke(
    session: Option<&str>,
    path: String,
    args: Value,
    window: Option<&str>,
) -> Value {
    let mut params = windowed_params(session, path, window);
    params["args"] = args;
    params
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
/// Every OTHER fault passes through untouched. This translates one refusal whose cause it can
/// state; dressing up faults it cannot explain would be the guess-as-fact this product spent R325
/// deleting from ten verbs.
///
/// # It takes the whole `params`, and that is what let the other sixteen readers in
///
/// It used to take a bare `path` and build `{"path": …}` itself, so the one shape it accepted was
/// the UNSCOPED read — and every other reader in this file passes [`scoped_params`],
/// [`windowed_params`] or [`site_params`]. That is why the register's *"one line each"* estimate
/// was wrong when R319 measured it: adoption was blocked by a signature, not by sixteen edits.
/// Taking the params the caller already built makes every call site a one-line change, which is
/// what it is now.
///
/// The COMMAND name is gone from the sentence with it. It said `processes: this daemon does not
/// serve …`, which reads well until a verb makes two reads — `ls` reads `sessions` and
/// `session_activity`, `agent` reads the manifests and the panes — and then the verb is the half
/// the person already typed while the ADDRESS is the half that says which read stopped. Keeping it
/// would have meant threading a command name through [`pane_ids`], [`window_names`] and
/// [`session_panes`], each shared by a dozen verbs, to print a word the shell line above the error
/// already shows.
fn query_slot(conn: &mut HostConn, params: Value) -> io::Result<Value> {
    // Read BEFORE the call, which consumes the params; the sentence needs the address that failed.
    let path = params["path"].as_str().unwrap_or_default().to_owned();
    match query_raw(conn, params) {
        Ok(value) => Ok(value),
        Err(CallError::Fault(fault)) => {
            Err(unknown_slot(&path, &fault).unwrap_or_else(|| CallError::Fault(fault).into()))
        }
        Err(other) => Err(other.into()),
    }
}

/// The ONE place this binary names the query method, so a reader added later cannot reach the
/// daemon without passing the site where the skew sentence is decided.
///
/// [`query_slot`] is what all but one caller wants. The exception is [`session_exists`], which
/// needs the fault itself because for it one particular refusal is an ANSWER rather than a failure.
fn query_raw(conn: &mut HostConn, params: Value) -> Result<Value, CallError> {
    conn.try_call("scene/query", params)
}

/// Send one `scene/invoke` and render its failure — the ONE door every acting verb goes through.
///
/// # The wrong answer this exists to stop
///
/// A refused invoke and an UNKNOWN invoke arrived under one JSON-RPC code until PINION-PR82, so a
/// verb that mapped every fault to its own sentence reported the user's arguments as the fault.
/// Measured against a peer that serves every read and knows no action (`AgedHost` in
/// `tests/cli.rs`), **twenty-one of the twenty-four acting verbs did exactly that**: `kill-session
/// 0` answered *no session named "0"* about a session the daemon was holding. R322 moved the guard
/// to this seam — the same move R321 made for the reading side ([`query_slot`]) — so the wrong
/// thing is not caught by a rule authors must remember; it is the thing the one available call does
/// not do.
///
/// # Three sentences, in the order of how much they tell a person
///
/// 1. **A SKEW** — the daemon does not perform that action at all ([`unknown_action`]), so no verb
///    reports a taken name to somebody whose daemon simply predates the verb (R297 measured exactly
///    that: `rename-session` said *"prod" is already another session's name* about a name no session
///    held).
/// 2. **A STATED REFUSAL** — the daemon HAD the action, declined, and said why
///    ([`sprag_host::wire::refusal`]). Its sentence is printed verbatim; this end adds nothing,
///    because a client improving on it would be authoring a claim about state it cannot see.
/// 3. **AN UNSTATED ONE** — a refusal carrying no reason, which on this build is unreachable (the
///    type requires the sentence) and on an older daemon is a version skew told as one
///    ([`sprag_host::wire::unstated_refusal`]).
///
/// # It took a per-verb closure until R325, and every caller now wants the same thing
///
/// There were TWO doors: this one and an `invoke_action_with` taking a sentence the verb wrote for
/// itself. Ten of those sentences were client-side DISJUNCTIONS — `join-pane` casting doubt on
/// three arguments when the daemon had rejected exactly one — and PINION-PR82 made every one of
/// them the daemon's to state instead. With the last of them gone the parameter had one value at
/// every call site, so the second door closed: one seam, and no verb can grow a guess without
/// re-opening it.
fn invoke_action(conn: &mut HostConn, params: Value) -> io::Result<Value> {
    // Read BEFORE the call, which consumes the params — [`query_slot`]'s reason exactly.
    let path = params["path"].as_str().unwrap_or_default().to_owned();
    match conn.try_call("scene/invoke", params) {
        Ok(answer) => Ok(answer),
        Err(CallError::Fault(fault)) => Err(unknown_action(&path, &fault)
            .or_else(|| sprag_host::wire::refusal(&fault))
            .unwrap_or_else(|| sprag_host::wire::unstated_refusal(&path))),
        Err(CallError::Transport(error)) => Err(error),
    }
}

/// `display-message [-t SESSION] [-c CLIENT] [-s note|warn|alert] MESSAGE` — put a sentence in
/// front of the people looking at this daemon (tmux `display-message`).
///
/// # What it is FOR, measured rather than assumed
///
/// Measured at `5acde43`: nothing in this product could put a word on a person's screen from outside
/// the client they were typing into. `report-agent` moves the terminal's window TITLE and carries a
/// three-word state; `send-keys` types the words INTO the person's program, which is a corruption of
/// their command line rather than a message; a pane child's OSC 9 reached the terminal front nowhere
/// at all. So an agent that finished a build in one pane had no way to say so to the person in
/// another.
///
/// # The two flags, and why they are `-t` and `-c` rather than one target
///
/// `-t` is this CLI's SESSION scope everywhere else, and it stays that here: it selects the default
/// audience — everyone attached to that session. `-c` names ONE client, wherever it is attached,
/// which is exactly tmux's split of the same two words. A `-c` naming a client that is not attached
/// is an ERROR rather than an empty delivery: a caller that named a target got the name wrong, which
/// is a different fact from *nobody is watching*, and answering them alike would send somebody
/// looking for a person who is right there.
///
/// # The answer
///
/// WHO it reached, in [`sprag_host::Delivery`]'s own words, and it EXITS 0 either way — including
/// when it reached nobody. That is a deliberate line: the request was well-formed and the daemon
/// carried it out; *nobody is attached* is an ANSWER, and printing it as one keeps the exit code
/// meaning what it means for every other verb here. A caller that must branch on it reads the
/// sentence, or uses the MCP tool, which answers the list as data.
fn display_message(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "display-message")?;
    let mut client: Option<String> = None;
    let mut severity: Option<String> = None;
    let mut text: Option<String> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-c" => {
                client = Some(
                    it.next()
                        .ok_or_else(|| bad_input("display-message: -c needs a client id"))?,
                );
            }
            "-s" => {
                let word = it
                    .next()
                    .ok_or_else(|| bad_input("display-message: -s needs a severity"))?;
                // Checked HERE and not left to the daemon's refusal, for `report-agent`'s reason one
                // verb over: a closed vocabulary is a thing a person mistypes, and `-s error` must
                // say what the words are rather than come back as a bare "invalid params".
                if sprag_host::report::Severity::parse(&word).is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "display-message: {word:?} is not a severity (it is one of {})",
                            sprag_host::report::Severity::words(),
                        ),
                    ));
                }
                severity = Some(word);
            }
            other if text.is_none() && !other.starts_with('-') => text = Some(other.to_owned()),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "display-message: unexpected argument {other:?} (display-message \
                         [-t SESSION] [-c CLIENT] [-s note|warn|alert] MESSAGE)"
                    ),
                ));
            }
        }
    }
    let text = text.ok_or_else(|| bad_input("display-message: needs a message to show"))?;
    // The same rules the daemon enforces, applied here so the caller is told WHICH one they broke.
    // The daemon still checks — this is a second reader of one grammar, never a second grammar — but
    // a wire refusal cannot carry a payload (PINION-PR82), so without this a newline in a message
    // comes back as an undifferentiated "invalid params".
    let text = sprag_host::report::MessageText::parse(&text).map_err(|why| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("display-message: {why}"),
        )
    })?;
    // Pre-flighted for [`runs`]' reason, and this verb is the one that proves the reason is not
    // about the RUN family: it carried the identical false skew sentence and is not one of them.
    let mut conn = connect_scoped(session.as_deref())?;
    let mut params = serde_json::json!({ "text": text.as_str() });
    if let Some(severity) = severity {
        params["severity"] = Value::String(severity);
    }
    if let Some(client) = &client {
        params["client"] = Value::String(client.clone());
    }
    let answer: Value = invoke_action(
        &mut conn,
        scoped_invoke(
            session.as_deref(),
            mux_action_path(DISPLAY_MESSAGE_ACTION),
            params,
        ),
        // Both causes this used to guess between — a client id that is not attached, and a text
        // the daemon will not paint — are STATED by the daemon now, and its version of the first
        // is better than the guess: it names who IS attached instead of naming a verb to run.
    )?;
    let reached: Vec<&str> = answer["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    match reached.as_slice() {
        [] => println!("shown to nobody: no client is attached"),
        [one] => println!("shown to {one}"),
        many => println!("shown to {} clients: {}", many.len(), many.join(", ")),
    }
    Ok(())
}

/// `report-agent STATE [-t SESSION] [--pane N] [--source S] [--name AGENT] [--seq N]
/// [--asked PROMPT] [--said ANSWER] [--transcript PATH]`: say what the agent in a pane is doing,
/// from INSIDE that pane.
///
/// ⚠⚠ THESE PARAGRAPHS SAT ON `display_message`, WHICH IS A DIFFERENT VERB. They were written above
/// that function with no blank line between them, so rustdoc read the whole lot as one comment about
/// `display-message` and this function had no doc at all. Found while adding `--asked` (register
/// item 730) — the new prose would have landed on the wrong verb too.
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
///
/// # ⛔⛔⛔⛔⛔ `--asked` IS WHAT LETS A REPORTER END A TURN AT ALL — register item 730
///
/// [`AGENT_ASKED_KEY`](sprag_host::wire::AGENT_ASKED_KEY) is the key
/// [`Tracker::asked_seq`](sprag_detect::Tracker::asked_seq) counts, and
/// [`DoneWhen::Settles`](sprag_plugin::DoneWhen::Settles) — the contract every shipped agent loop
/// runs on ([`INNER_SESSION_ENDS`](sprag_plugin::INNER_SESSION_ENDS)) — **pairs a peer's rest
/// against that counter** wherever the rest is the agent's own statement
/// ([`Authority::is_exact`](sprag_plugin::Authority::is_exact)). So a reporter that cannot state a
/// prompt can report `idle` for ever and **the turn never ends**: the loop waits out its bound, on
/// every turn, for the whole life of the run.
///
/// ⚠⚠ The wire has carried this key since register item 441 and the daemon has always accepted it;
/// **only `sprag hook` could send one**. That made the shipped turn contract reachable exclusively
/// by the agents whose hooks this binary ships, and unreachable by any other reporter — a person, a
/// stand-in, an agent whose config sprag does not write. Measured 2026-08-27 (item 730): a
/// whole-run host gate could not drive `INNER_SESSION_ENDS` at all and had to fall back to
/// [`DoneWhen::Exits`](sprag_plugin::DoneWhen::Exits), so reflection, session replacement,
/// stand-down and the context ceiling were measured only over doubles.
///
/// ⚠ OMITTED WHEN NOT GIVEN, never sent as `null` — the hook's own rule one function down: most
/// reports say nothing about a prompt because they are not the event that opens a turn, and a
/// `null` would be a claim that the agent stated it had been asked nothing.
///
/// # ⛔⛔⛔⛔⛔ `--said` IS `--asked`'s OTHER END, AND IT DECIDES WHAT A LOOP CONVERGES ON
///
/// [`AGENT_SAID_KEY`](sprag_host::wire::AGENT_SAID_KEY) is what the agent states it ANSWERED, and
/// `OuterLoop::said_marker` — the reader that decides whether a milestone was declared — asks the
/// peer BEFORE it reads the pane. Register item 441 is what the pane costs when it is the only
/// road: measured 2026-08-18 at every judgement of a live run, the pane's whole logical-line count
/// stood at **37 and never moved** while the agent wrote reply after reply with the marker alone on
/// a row, because a full-screen agent repaints instead of scrolling. **Nine judged turns read as a
/// peer that said nothing.**
///
/// So a reporter that cannot state an answer leaves every convergence, every stand-down and every
/// reflection resting on a reader that goes permanently blind against the agents this loop is built
/// to drive. Like its two neighbours, the key has been on the wire and accepted by the daemon since
/// item 441, with `sprag hook` as its only writer.
///
/// # ⛔⛔⛔⛔ `--transcript` IS THE SAME ABSENCE, ONE KEY OVER — register item 730's residue
///
/// [`AGENT_TRANSCRIPT_KEY`](sprag_host::wire::AGENT_TRANSCRIPT_KEY) is where an agent says it is
/// WRITING, and it is what every reading of a run's spend is resolved through — a session that
/// states none reads `context`, `cold` and `floor` as `0`, which the document defines as *do not
/// decide on this*. Register item 431 measured a live loop composing a paragraph of arithmetic out
/// of that zero.
///
/// It had the same single writer `--asked` had, and the same consequence: **nothing but a
/// sprag-installed hook could make a run's spend readable at all**, so `reviewing`'s economics, the
/// context ceiling and register item 719's per-turn `produced` reading were unreachable by any
/// other reporter. Stated beside `--asked` because the hook states the two together: a transcript
/// path arrives on the event that OPENS a turn.
fn report_agent(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "report-agent")?;
    let mut state: Option<String> = None;
    let mut pane: Option<String> = None;
    let mut source: Option<String> = None;
    let mut name: Option<String> = None;
    let mut seq: Option<u64> = None;
    let mut asked: Option<String> = None;
    let mut said: Option<String> = None;
    let mut transcript: Option<String> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pane" => pane = Some(named_pane(&mut it, "report-agent")?),
            // ⚠⚠⚠⚠ WHERE THIS AGENT SAYS IT IS WRITING — `--asked`'s neighbour, missing for the
            // same reason and costing the same kind of blindness. See the doc above.
            "--transcript" => {
                transcript = Some(
                    it.next()
                        .ok_or_else(|| bad_input("report-agent: --transcript needs a path"))?,
                );
            }
            // ⚠⚠⚠⚠⚠ AND WHAT IT ANSWERED — `--asked`'s other end, and the LAST of the three keys
            // only `sprag hook` could send. See the doc above.
            "--said" => {
                said = Some(
                    it.next()
                        .ok_or_else(|| bad_input("report-agent: --said needs the answer text"))?,
                );
            }
            // ⚠⚠⚠⚠⚠ THE PROMPT THIS TURN OPENED ON — register item 730, and the one argument here
            // that changes what a DRIVER can do rather than what a reader sees. See the doc above.
            "--asked" => {
                asked = Some(
                    it.next()
                        .ok_or_else(|| bad_input("report-agent: --asked needs the prompt text"))?,
                );
            }
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
                         [-t SESSION] [--pane N] [--source S] [--name AGENT] [--seq N] \
                         [--asked PROMPT] [--said ANSWER] [--transcript PATH])"
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
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_optional_pane(
        &mut conn,
        session.as_deref(),
        pane.as_deref(),
        "report-agent",
    )?;
    let pane = match &site {
        Some(site) => site.id,
        None => own_pane("report-agent")?,
    };
    let mut params = serde_json::json!({
        "id": pane,
        "state": state,
        "source": source.unwrap_or_else(|| "cli".to_owned()),
        // ⚠⚠⚠ WHICH BUILD IS REPORTING, stated on every report and by THIS process about ITSELF.
        //
        // The reporter is the hook binary, which a `cargo build` replaces under a running daemon —
        // under every running daemon at once — while the daemon goes on being whatever was started.
        // Until this key the skew was unobservable from either end: the reports are accepted, the
        // verdicts look right, and the code producing them is not the code the daemon is.
        //
        // ⚠ It is sent unconditionally rather than only when it might differ, because this side
        // cannot know: `HostConn::daemon_build` is what the DAEMON said about itself, and a
        // comparison made here would answer for one connection while the daemon has to answer for
        // every reporter it has. Stating the fact and letting the holder of both compare is the same
        // division `source` follows. See `sprag_host::wire::AGENT_BUILD_KEY`.
        sprag_host::wire::AGENT_BUILD_KEY: sprag_host::wire::BUILD,
    });
    if let Some(name) = name {
        params["name"] = Value::String(name);
    }
    if let Some(seq) = seq {
        params["seq"] = Value::from(seq);
    }
    // ⚠⚠⚠⚠⚠ SENT ONLY WHEN STATED, on the hook's own rule (see `deliver_hook`): a `null` here would
    // be a claim that the agent said it had been asked nothing, and most reports are not the event
    // that opens a turn. Absent, `Tracker::report` leaves `asked_seq` where it was — which is
    // exactly the reading `DoneWhen::Settles` pairs a rest against.
    if let Some(asked) = asked {
        params[sprag_host::wire::AGENT_ASKED_KEY] = Value::String(asked);
    }
    // ⚠⚠ ON THE SAME TERMS AS ITS TWO NEIGHBOURS: sent only when stated. The hook sends this one on
    // the event that ENDS a turn, where `--asked` rides the one that opens it.
    if let Some(said) = said {
        params[sprag_host::wire::AGENT_SAID_KEY] = Value::String(said);
    }
    // ⚠⚠ ON THE SAME TERMS, and the hook states the two together for a reason: a transcript path
    // arrives on the event that OPENS a turn, so a report carrying one without a prompt is a claim
    // nothing else in this product makes.
    if let Some(transcript) = transcript {
        params[sprag_host::wire::AGENT_TRANSCRIPT_KEY] = Value::String(transcript);
    }
    // The refusal stays a REFUSAL rather than becoming a rendered sentence this side would then
    // have to match on ([`agent_refusal`]). A transport failure is passed through untouched: it is
    // not about panes, and dressing it as if it were would be the same class of wrong answer this
    // replaces — and so is a SKEW, which [`invoke_action`] has already taken out of the fault
    // by the time the sentence below reasons about detectors.
    let answer: Value = invoke_action(
        &mut conn,
        match &site {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(REPORT_AGENT_ACTION),
                params,
            ),
            None => scoped_invoke(
                session.as_deref(),
                mux_action_path(REPORT_AGENT_ACTION),
                params,
            ),
        },
    )?;
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
    let mut pane: Option<String> = None;
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
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_optional_pane(
        &mut conn,
        session.as_deref(),
        pane.as_deref(),
        "release-agent",
    )?;
    let pane = match &site {
        Some(site) => site.id,
        None => own_pane("release-agent")?,
    };
    let answer: Value = invoke_action(
        &mut conn,
        match &site {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(RELEASE_AGENT_ACTION),
                serde_json::json!({ "id": pane }),
            ),
            None => scoped_invoke(
                session.as_deref(),
                mux_action_path(RELEASE_AGENT_ACTION),
                serde_json::json!({ "id": pane }),
            ),
        },
    )?;
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

/// **LEAVE WORD THAT THIS PANE'S REPORTER IS MUTE, OR TAKE THE WORD BACK** — see
/// [`hooks::note_mute`] for what an hour of silence cost, why the file's EXISTENCE is the message,
/// and why it now names the generation it was left in.
///
/// ⚠⚠⚠ ONLY REACHED ONCE [`sprag_host::PANE_ENV_VAR`] HAS RESOLVED, which is the line the swallow
/// rule already drew and did not use: a session with nothing to do with sprag never gets this far,
/// so nothing here can make a stranger's agent noisy. That is the whole argument for writing
/// anything at all.
///
/// ⚠ Its own failures are swallowed exactly as the report's are. A hook that could not report AND
/// could not say so is where this started, but a hook that PANICS because a state directory is
/// read-only would be worse than the silence — it runs in the agent's critical path.
///
/// ⚠⚠⚠⚠ **THE WORD NAMES ITS OWN SUBJECT** — register item 711. The file is keyed on a pane NUMBER
/// and the next generation's counter hands that number out again, so a breadcrumb with no generation
/// in it is read against whoever inherits the number: measured on this host, `hook-mute.4` from
/// 14:02 against a live pane 4 whose child started at 22:57. The generation comes out of the pane's
/// own environment ([`sprag_host::PANE_GENERATION_ENV_VAR`]) rather than off the wire, because this
/// path exists for the case where the daemon cannot be reached at all.
///
/// ⚠ `None` there is *this reporter cannot say which generation it belongs to*, which a reader
/// treats as unattributable rather than as a match. It reaches that only in a pane born without the
/// variable, which no daemon serving a socket produces.
fn note_hook_trouble(pane: u64, trouble: Option<&str>) {
    sprag_host::hooks::note_mute(
        // ⚠ The one production caller says the directory OUT LOUD — item 700's ruling, now on the
        // writing side as well as the reading one.
        &sprag_host::durability::state_dir(),
        pane,
        std::env::var(sprag_host::PANE_GENERATION_ENV_VAR)
            .ok()
            .filter(|generation| !generation.is_empty())
            .as_deref(),
        trouble,
    );
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
    let payload: Value = serde_json::from_str(&payload).ok()?;
    let outcome = hooks::report_for(target, &payload)?;
    // ⚠⚠⚠⚠ WHAT THE AGENT STATES, ALONGSIDE WHAT IT MEANS — see [`hooks::asked_in`]. Read here and
    // not inside `report_for` because the two are different kinds of thing: one is this crate's
    // decision about a state, the other is the agent's own account of its turn, and only the second
    // can settle whether a prompt arrived.
    let asked = hooks::asked_in(&payload);
    // ⚠⚠⚠⚠ AND WHAT IT ANSWERED, off the event that ends the turn — see [`hooks::said_in`]. Read
    // beside `asked` and for the same reason: the two ends of one turn are the agent's own account
    // of it, and neither is a state this crate decides. Register item 441 is what the answering
    // half costs when nobody reads it: the pane it would otherwise be scraped off was measured
    // unable to advance its line addresses at all.
    let said = hooks::said_in(&payload);
    // ⚠⚠⚠⚠ AND WHY IT WANTS A PERSON, off the one event it raises to ask for one — see
    // [`hooks::noticed_in`]. The third of the same kind: `report_for` decides a STATE and these three
    // carry what the agent itself stated. Until this line the whole notice was reduced to the word
    // `blocked` and a run that stopped for a person could not tell them what for, while the sentence
    // had been in this process's stdin (register item 452).
    let noticed = hooks::noticed_in(&payload);
    // ⚠⚠⚠⚠ AND WHAT IT IS ABOUT TO RUN, off the one event that OPENS a tool call — see
    // [`hooks::running_in`]. The fourth of the same kind, and the one that fills the gap the report
    // COUNTER leaves: that counter moves per event, so a tool call longer than a waiter's silence
    // bound reads exactly like a turn nothing will ever speak for again. Register item 721 is a run
    // killed inside such a gap while `cargo check` was on its screen.
    let running = hooks::running_in(&payload);
    let pane = std::env::var(sprag_host::PANE_ENV_VAR)
        .ok()?
        .parse::<u64>()
        .ok()?;
    // ⚠⚠⚠ FROM HERE ON A FAILURE IS SPRAG'S OWN, AND IS RECORDED. `PANE_ENV_VAR` has resolved, so
    // this is a pane this daemon made — the swallow rule's *"a session with nothing to do with
    // sprag"* case is already behind us, and it is the only case that rule argues about. See
    // `note_hook_trouble`.
    //
    // The bound is stated to `connect`, not set after it: the handshake it performs is itself a
    // wait, and this one runs while an agent holds still for it.
    let mut conn = match connect_within(HOOK_DEADLINE) {
        Ok(conn) => conn,
        Err(why) => {
            note_hook_trouble(pane, Some(&format!("could not reach the daemon: {why}")));
            return None;
        }
    };
    let (action, params) = match outcome {
        hooks::Outcome::Report(state) => (
            REPORT_AGENT_ACTION,
            json!({
                "id": pane,
                "state": state.wire_str()?,
                "source": format!("hook:{}", target.name),
                "name": target.agent,
                "seq": hooks::report_seq()?,
                // ⚠⚠⚠ THE TWO FACTS ONLY THE AGENT KNOWS, sent when it stated them and OMITTED
                // otherwise — `null` would be a claim that it said nothing, and most events say
                // nothing about a prompt because they are not the event that opens a turn.
                sprag_host::wire::AGENT_ASKED_KEY: asked.as_ref().map(|a| a.prompt.clone()),
                sprag_host::wire::AGENT_SAID_KEY: said.clone(),
                sprag_host::wire::AGENT_NOTICED_KEY: noticed.clone(),
                sprag_host::wire::AGENT_RUNNING_KEY: running.clone(),
                sprag_host::wire::AGENT_TRANSCRIPT_KEY: asked
                    .as_ref()
                    .and_then(|a| a.transcript.as_ref())
                    .map(|path| path.display().to_string()),
                // ⚠⚠⚠⚠⚠ WHICH BUILD IS REPORTING — register item 459. THIS is the reporter the key
                // was written for: `AGENT_BUILD_KEY`'s whole argument is that a `cargo build`
                // replaces THE HOOK BINARY under a running daemon, "the ORDINARY state after any
                // rebuild", and until this line the hook was the one reporter that never said. The
                // only writer in the tree was the `report-agent` VERB — a person at a command line,
                // the reporter the key was NOT written for — so `reporter_build` was `None` for
                // every production report, which that key's own doc defines as *this reporter did
                // not say*. Item 412's quiet skew had no detector at all.
                //
                // ⚠ On the same terms `report-agent` states it: unconditionally, and about THIS
                // process rather than about the daemon. This side cannot make the comparison —
                // `HostConn::daemon_build` is one connection's answer where the daemon has to
                // answer for every reporter it holds — so it states the fact and lets the holder of
                // both compare, exactly as `source` divides the same work.
                sprag_host::wire::AGENT_BUILD_KEY: sprag_host::wire::BUILD,
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
    // ⚠⚠⚠ THIS IS THE CALL THE WIRE SKEW REFUSES, and the refusal is the sentence worth keeping:
    // `client/hello` names both the problem and its fix, and until now it was written where nobody
    // could read it. A report that lands takes the breadcrumb back.
    match invoke_action(
        &mut conn,
        scoped_invoke(None, mux_action_path(action), params),
    ) {
        Ok(Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_))
        | Ok(Value::Array(_) | Value::Object(_)) => note_hook_trouble(pane, None),
        Err(why) => {
            note_hook_trouble(pane, Some(&format!("the daemon refused the report: {why}")));
            return None;
        }
    }
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

/// The `--pane` value for a verb that takes one, as the caller SPELLED it — an id or a NAME.
///
/// Not parsed into a number here: telling a name from an id is
/// [`PaneAddress`]'s decision, and resolving one needs a
/// connection this parse does not have. A verb calls [`resolve_pane`] once it is connected.
fn named_pane(it: &mut impl Iterator<Item = String>, command: &str) -> io::Result<String> {
    it.next()
        .ok_or_else(|| bad_input(&format!("{command}: --pane needs a pane id or NAME")))
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
    // ⛔⛔⛔ A PANE NAMES THE WINDOW — register item 782, the same repair as [`layout`]'s and for
    // the same reason: this listing could only ever be the caller's own window's.
    let named = one_pane_at_most(&rest, "panes")?;
    let mut conn = connect_scoped(session.as_deref())?;
    let elsewhere = read_target_window(&mut conn, session.as_deref(), named, "panes")?;
    let listed: Value = query_slot(
        &mut conn,
        match elsewhere.as_deref() {
            Some(window) => windowed_params(
                session.as_deref(),
                mux_action_path(PANES_SLOT),
                Some(window),
            ),
            // ⛔⛔⛔ THROUGH `here_params` ALL THE SAME — register item 759, unchanged. Reading the
            // slot directly is about the ROW, never about the window, and sending a different
            // scope from `pane_ids` would make this listing and every id check answer about two
            // different windows.
            None => here_params(session.as_deref(), mux_action_path(PANES_SLOT)),
        },
    )?;
    // The whole entry, not just the id — this is the one command whose subject is the LIST, so it
    // reads the slot directly rather than through `pane_ids`.
    //
    // ⛔⛔⛔ THROUGH `here_params` ALL THE SAME — register item 759. Reading the slot directly is
    // about the ROW, never about the window, and sending a different scope from `pane_ids` would
    // make this listing and every id check answer about two different windows. That is the drift
    // `pane_ids`' own doc promises cannot happen, and one helper is how.
    for pane in listed.as_array().into_iter().flatten() {
        println!("{}", pane_row(pane));
    }
    Ok(())
}

/// ONE pane's row of [`panes`], built as a string rather than printed — so something other than a
/// human eye can read what a person reads.
///
/// ⚠⚠⚠⚠⚠ **IT IS A FUNCTION BECAUSE THIS ROW HAD NO OTHER READER** — register item 595, and the
/// shape register item 418 already paid for here: a fact can ride the wire, be asserted at the
/// daemon, and still never reach the listing a person greps, because nothing in the suite has ever
/// looked at that listing. Every marker below is additive and absent by default, which keeps the
/// row byte-identical for every script written against it — and is exactly why an omission here is
/// invisible until somebody reports the product as broken.
fn pane_row(pane: &Value) -> String {
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
    // ⚠⚠⚠⚠⚠ WHETHER THE PANE'S CHILD IS STILL THERE — register item 418, and the omission cost
    // a person their model of the product. A dead pane printed byte-identically to a live one,
    // so somebody typed `Esc` at a terminal with no program on it, watched nothing happen, and
    // reported that the KEY was broken. The daemon had known all along: `dead` rides this very
    // row (additive and one-way), the GUI's title has said it since the marker existed, and
    // THIS listing — the one a person greps — dropped it on the floor.
    //
    // ⚠⚠ The words are `sprag_terminal::exit_phrase`'s, not spelled again here: the GUI and this
    // are two placings of one vocabulary, and a second spelling is how a signalled death becomes
    // `exited 1` on one surface and `killed: Terminated` on the other.
    //
    // ⚠ LAST on the row, after `(active)`, because it is the most final thing that can be said
    // about a pane — the GUI's title orders the same three facts the same way, by increasing
    // finality — and because a dead pane can still be the ACTIVE one, so the two do not compete
    // for a position. Absent for a live pane, which keeps this listing byte-identical to the
    // shape every script parsing it was written against.
    //
    // ⚠ `child_exit` is rebuilt from its two published fields rather than deserialised: the
    // daemon writes `{code, signal?}` by hand there (`PaneExit` carries no serde derive, on
    // purpose — it crosses the wire as a shape the host owns), and a pane can be `dead` with no
    // `child_exit` at all, which is the not-yet-reaped state `exit_phrase` takes `None` for.
    let exited = if pane["dead"] == json!(true) {
        let how = pane["child_exit"]
            .as_object()
            .map(|exit| sprag_terminal::PaneExit {
                code: exit.get("code").and_then(Value::as_u64).unwrap_or(0) as u32,
                signal: exit
                    .get("signal")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        format!("  ({})", sprag_terminal::exit_phrase(how.as_ref()))
    } else {
        String::new()
    };
    // ⚠⚠⚠⚠⚠ **HOW THIS PANE WAS BORN, AND WHETHER ANYTHING HAS IT NOW** — register items 595
    // and 602, and the pair only says anything TOGETHER.
    //
    // A daemon restart re-runs an allowlisted agent's argv, so a `claude` pane comes back
    // holding its old conversation; and it comes back UNDRIVEN, because a restart brings panes
    // back alive and runs back ended. On this listing that pane used to print byte-identically
    // to the `claude` a person opened on purpose and to the one a loop is mid-turn in — three
    // very different things, one row. Measured 2026-08-22, three times in one day, the last
    // with `sprag runs` reporting no running run beside three live `claude` processes.
    //
    // ⚠⚠ ORDERED THE WAY THE PANE HAPPENED: who asked for it, how it was born, what holds it
    // now. `(revived)` with no `(driven)` beside it is the orphan — an agent nobody asked for,
    // holding tokens and context — and reading that off one row is the whole point of putting
    // the two markers next to each other rather than on separate verbs.
    //
    // ⚠ Both absent rather than `false`, which is the rule the wire keys state and the reason
    // is the same one step later: a `(not driven)` on every shell in the workspace is noise on
    // the common path, and noise is what gets skimmed past on the row that matters.
    let revived = if pane[PANE_REVIVED_KEY] == json!(true) {
        "  (revived)"
    } else {
        ""
    };
    let driven = if pane[PANE_DRIVEN_KEY] == json!(true) {
        "  (driven)"
    } else {
        ""
    };
    // ⛔⛔⛔⛔⛔ **AND WHO IS LIVING IN IT, WHICH IS WHAT A PERSON ABOUT TO CLOSE IT NEEDS** —
    // register item 865's ⑸.
    //
    // # ⚠⚠⚠⚠⚠ The two markers above answer about the PANE; this answers about its occupant
    //
    // `(revived)` says how the pane was born and `(driven)` says whether a run holds it now, and
    // both were checked — along with `sprag runs` — before two panes were closed on 2026-09-03.
    // All three were about runs and panes. **Two live Claude conversations died with that window**,
    // and the first sign was `No agent named 'mnemosyne-2b' is reachable` afterwards. The
    // conversation was in the daemon's own pane record the entire time; this listing simply never
    // said it, so the question was not answered wrongly — it could not be asked.
    //
    // ⚠⚠ IT IS THE ID AND NOT A COUNT, because the id is the JOIN: the names a person sees in
    // their agent roster are labels the daemon has never heard of, and a conversation id is what
    // ties a row here to one of them (a session's own scratchpad path carries it). A bare *this
    // pane has an agent* would say the pane is not empty and leave a reader unable to find out
    // whose it is, which is the half of that morning that actually hurt.
    //
    // ⚠ Absent for a shell, its neighbours' rule: a key on every pane in the workspace is noise on
    // the common path, and the common path is most of this listing.
    let session = pane[sprag_host::wire::PANE_SESSION_KEY]
        .as_str()
        .map_or_else(String::new, |id| format!("  session={id}"));
    format!(
        "{id}: {cols}x{rows}  \
         {command}{name}{title}{opened_by}{session}{revived}{driven}{active}{exited}"
    )
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
    // ⛔⛔⛔ A PANE NAMES THE WINDOW — register item 782. This used to refuse every positional and
    // could only ever draw the window the scope was already on. See [`read_target_window`].
    // ⚠ The COUNT is checked before the socket, which is the property the existing gate pinned.
    let named = one_pane_at_most(&rest, "layout")?;
    let mut conn = connect_scoped(session.as_deref())?;
    let elsewhere = read_target_window(&mut conn, session.as_deref(), named, "layout")?;
    let answer: Value = query_slot(
        &mut conn,
        match elsewhere.as_deref() {
            Some(window) => windowed_params(
                session.as_deref(),
                mux_action_path(LAYOUT_SLOT),
                Some(window),
            ),
            // ⚠⚠ TWO DIFFERENT `None`s, and they are not the same request — register item 782.
            // A caller who NAMED nothing gets this verb's own subject, the scope's current window.
            // A caller who named a pane and got `None` back named one of THEIR OWN
            // ([`PaneSite::window`]), so the honest answer is their window and not the current one
            // — the two are the same only for a caller standing in no pane at all.
            None if named.is_some() => {
                here_params(session.as_deref(), mux_action_path(LAYOUT_SLOT))
            }
            None => scoped_params(session.as_deref(), mux_action_path(LAYOUT_SLOT)),
        },
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

/// `processes [PANE] [-t SESSION] [-a]`: WHAT EACH PANE IS RUNNING — its terminal device, the child
/// the daemon spawned, and the job that currently owns its terminal, with every process in that job.
///
/// The third of the pane verbs and the one the other two cannot answer: [`panes`] says WHO (and its
/// `command` is the label a pane was SPAWNED with, frozen at birth — a pane opened as `bash` and now
/// three hours into a `cargo build` still lists as `bash`), [`layout`] says WHERE, and this says what
/// is actually running. A shell hands its terminal to the job it starts and takes it back when that
/// job ends, so the foreground group is the OS's own answer to "what did the user set going", and
/// until this verb existed nothing outside the daemon could ask for it.
///
/// # Why the reading is registry-wide and the ANSWER is not
///
/// The daemon's answer is registry-wide by construction and pretending otherwise would cost
/// something: `/proc` carries no index by process group, so enumerating ONE pane's job is the same
/// full pass that answers every other pane. Narrowing at the daemon would mean either a second slot
/// read to learn which ids are in scope, or two scopes each paying the same walk.
///
/// So the walk stays whole and the NARROWING is here, in the client, where it costs a filter —
/// which is why this verb can take `-t SESSION` and mean by it exactly what every other pane verb
/// means, and take `[PANE]` and mean one pane, off the same reading. `-a` asks for the reading as
/// it was taken, every session at once, and is the answer to *which pane on this machine is eating
/// it*. [`pane_and_scope`] holds the parse and the measurement of what this used to do instead.
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
    let scope = pane_and_scope(args, "processes")?;
    // Scoped, so a session nobody has is a clean *no session named* rather than an answer about
    // the whole machine — the pre-flight `sprag panes` has always made, and the second half of the
    // defect [`pane_and_scope`] records.
    let mut conn = connect_scoped(scope.session.as_deref())?;
    // The READING is registry-wide and the NARROWING is client-side — which is what lets `-t` mean
    // the same thing here as everywhere else without a second walk of `/proc`.
    let narrowing = scope.narrowing(&mut conn, "processes")?;
    let reading = query_slot(
        &mut conn,
        json!({ "path": mux_action_path(&pane_processes_at(0)) }),
    )?;
    let wire: PaneProcessesWire = serde_json::from_value(reading).map_err(|error| {
        bad_input(&format!(
            "processes: the host's answer did not parse: {error}"
        ))
    })?;
    let rows: Vec<_> = wire
        .panes
        .iter()
        .filter(|row| narrowing.holds(row.id))
        .collect();
    if let Some(id) = narrowing.named_pane()
        && rows.is_empty()
    {
        // The caller ASKED about that pane, so silence would be the wrong answer — the same rule
        // [`resolve_pane`] follows for every verb that takes a target.
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

/// `VERB [PANE] [-t SESSION] [-a]`: what a registry-wide READING is narrowed to before it prints,
/// for the two verbs that take one — [`processes`] and [`resources`].
///
/// # The two defects this exists to have fixed
///
/// `processes` shipped at R290 with a usage line reading `processes [PANE] [-t SESSION]` and a
/// parser that refused any second argument, so `sprag processes buildout -t work` answered
/// *"unexpected argument \"-t\""* about a flag its own `--help` promised. Measured against a live
/// daemon at R338, when the new verb copied the parser along with the usage — which is what makes
/// this one function rather than two: the two verbs make the same claim in `--help`, so they must
/// take the same arguments, and a shared parser is the only shape where that cannot drift.
///
/// Then the flag was accepted and DROPPED. Both readings cover the whole registry, and the
/// narrowing was by PANE alone — so with no pane named the scope reached nothing, and every session
/// got the same answer. Measured on a live daemon on 2026-08-17, session `0` holding panes 0 and 2
/// and `work` holding 1 and 3:
///
/// ```text
/// sprag panes     -t 0      -> 0                     sprag panes -t work   -> 1  3
/// sprag processes -t 0      -> 0  2  1  3            sprag processes -t work -> 0  2  1  3
/// sprag processes -t nosuch -> 0  2  1  3            sprag panes -t nosuch -> no session named
/// ```
///
/// The doc above this function said so in as many words — *"the scope is NOT a filter on the
/// answer"* — which made a silently-ignored flag read as a decision. It was not one: `-t` means the
/// same thing on every other pane verb, and a verb that publishes it and then answers about the
/// machine is telling the caller their scope was heard.
///
/// # Why the machine-wide answer needed a WORD
///
/// It is a real question — *which pane on this box is eating the CPU* — and the pane eating it may
/// well be in a session the caller is not scoped to, which is why the reading is registry-wide in
/// the first place. Narrowing without giving that answer a spelling would have traded one lost
/// capability for another, so `-a` is it: tmux's own `list-panes -a`, the same letter for the same
/// meaning.
///
/// `-a` WITH a pane is refused here rather than resolved one way, because the two are a
/// contradiction (*every pane*, and *this one*) and picking a winner silently is the failure this
/// whole shape exists to remove.
fn pane_and_scope(args: Vec<String>, verb: &str) -> io::Result<PaneScope> {
    let (session, rest) = scope_and_rest(args, verb)?;
    let mut pane = None;
    let mut everywhere = false;
    for arg in rest {
        match arg.as_str() {
            "-a" | "--all" => everywhere = true,
            _ if pane.is_none() => pane = Some(arg),
            other => {
                return Err(bad_input(&format!(
                    "{verb}: unexpected argument {other:?} ({verb} [PANE] [-t SESSION] [-a])"
                )));
            }
        }
    }
    if everywhere && let Some(named) = &pane {
        return Err(bad_input(&format!(
            "{verb} -a answers about every pane on this daemon, so it cannot also be narrowed to \
             {named:?} — drop one of them"
        )));
    }
    Ok(PaneScope {
        session,
        pane,
        everywhere,
    })
}

/// What [`pane_and_scope`] parsed: which session, which pane, and whether the caller asked past
/// both.
struct PaneScope {
    /// The `-t SESSION` the caller named, or [`None`] for the session they are standing in (and
    /// failing that, the daemon's default).
    session: Option<String>,
    /// The `PANE` the caller named, as they SPELLED it — an id or a name, told apart by
    /// [`resolve_pane`].
    pane: Option<String>,
    /// `-a`: every pane the daemon has, whatever session holds it.
    everywhere: bool,
}

impl PaneScope {
    /// Which rows of a registry-wide reading this scope keeps.
    ///
    /// The order is the answer to *"what did the caller ask about"*, most specific first, with ONE
    /// exception that is the point of it: `-a` is read BEFORE the ambient pane. A caller standing
    /// in a pane has one resolved for them ([`resolve_optional_pane`]), so testing the pane first
    /// would let `sprag processes -a` run inside a pane mean *this pane* — a flag accepted and
    /// dropped, which is the defect one function up. An EXPLICIT pane cannot reach here beside
    /// `-a`; [`pane_and_scope`] refused that pair already.
    fn narrowing(&self, conn: &mut HostConn, verb: &str) -> io::Result<Narrowing> {
        if self.everywhere {
            return Ok(Narrowing::Everywhere);
        }
        if let Some(site) =
            resolve_optional_pane(conn, self.session.as_deref(), self.pane.as_deref(), verb)?
        {
            return Ok(Narrowing::Pane(site.id));
        }
        // Every WINDOW of the scoped session, which is the reach a pane argument already has here
        // ([`resolve_pane`]) — so the two spellings of "in this session" cannot disagree. It is
        // strictly more than `sprag panes`, which answers about one window because a display client
        // projects one window.
        Ok(Narrowing::Session(
            session_panes(conn, self.session.as_deref())?
                .into_iter()
                .map(|(_, id, _)| id)
                .collect(),
        ))
    }
}

/// Which panes of a registry-wide reading get printed.
///
/// A closed set rather than an `Option<u64>` plus a flag: the three answers are what the caller can
/// ASK for, and the two verbs that render them must not each work out the combination themselves.
enum Narrowing {
    /// One pane — the caller named it, or is standing in it.
    Pane(u64),
    /// Every pane the scoped session holds, in any of its windows.
    Session(Vec<u64>),
    /// Every pane on the daemon — `-a`, the reading as it is taken.
    Everywhere,
}

impl Narrowing {
    /// Whether a row for pane `id` belongs in the answer.
    fn holds(&self, id: u64) -> bool {
        match self {
            Self::Pane(pane) => *pane == id,
            Self::Session(ids) => ids.contains(&id),
            Self::Everywhere => true,
        }
    }

    /// The pane the caller NAMED, for the one refusal that is only owed to a caller who named one:
    /// silence is the right answer to a scope that holds nothing and the wrong answer to a pane.
    fn named_pane(&self) -> Option<u64> {
        match self {
            Self::Pane(id) => Some(*id),
            Self::Session(_) | Self::Everywhere => None,
        }
    }
}

/// `resources [PANE] [-t SESSION] [-a]`: what each pane is TAKING of the machine.
///
/// Scoped exactly as [`processes`] is, through the same [`pane_and_scope`] — and `-a` earns its
/// keep here most of all, because the pane starving this one may be in a session the caller has
/// never scoped to.
///
/// # Two numbers, printed together, because either alone misleads
///
/// The cores a pane is holding cannot be read on their own: a pane holding a tenth of a core is
/// either a pane with nothing to do or a pane being starved of what it asked for, and those want
/// opposite responses from a person. So every row prints what the pane GOT and what it WAITED for,
/// and the waiting column is not omitted when it is zero — a pane that took a little and waited for
/// nothing is the answer *this pane is idle*, which is information.
///
/// # Why it may pause before it prints
///
/// A rate needs two samples, and a daemon nobody has asked yet has one. [`settled`] is the shared
/// answer to that — one extra read after `SETTLE`, and the same one `sprag-mcp` makes, so the CLI
/// and an agent cannot come to disagree about how long is long enough.
fn resources(args: Vec<String>) -> io::Result<()> {
    let scope = pane_and_scope(args, "resources")?;
    let mut conn = connect_scoped(scope.session.as_deref())?;
    // The READING is registry-wide — a machine is not divided by session, and the pane eating it may
    // be in one the caller is not scoped to, which is what `-a` is for — so the narrowing is
    // client-side, exactly as `processes` does it.
    let narrowing = scope.narrowing(&mut conn, "resources")?;
    let wire = settled(|| {
        let reading = query_slot(
            &mut conn,
            json!({ "path": mux_action_path(&pane_resources_at(0)) }),
        )?;
        serde_json::from_value::<PaneResourcesWire>(reading).map_err(|error| {
            bad_input(&format!(
                "resources: the host's answer did not parse: {error}"
            ))
        })
    })?;
    let rows: Vec<_> = wire
        .panes
        .iter()
        .filter(|row| narrowing.holds(row.id))
        .collect();
    if let Some(id) = narrowing.named_pane()
        && rows.is_empty()
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "resources: no pane {id} (panes: {:?})",
                wire.panes.iter().map(|row| row.id).collect::<Vec<_>>()
            ),
        ));
    }
    println!("sampled {} ms ago", wire.sampled_ms_ago);
    for row in rows {
        match row.taken {
            Taken::Measured {
                cpu,
                waiting,
                memory,
                processes,
                granted,
            } => println!(
                "{}: {}  waiting {}  {}  {}  weight {}",
                row.id,
                held(cpu),
                waited(waiting),
                of(footprint(memory), granted.memory, footprint_ceiling),
                of(count(processes), granted.processes, count_ceiling),
                weight(granted.share),
            ),
            // The reason, not a blank row: "nothing on this machine is measured" and "this one pane
            // is not" send a reader in opposite directions.
            Taken::Unmeasured { reason } => println!("{}: {reason}", row.id),
        }
    }
    Ok(())
}

/// `grant <PANE> [--share N] [--memory MIB] [--processes N] [-t SESSION]`: what ONE pane is ALLOWED
/// of the machine.
///
/// `resources` says what each pane TOOK; this is the other half — what a person says it MAY take.
/// The machine's own `pane-memory-limit` / `pane-process-limit` are what every pane is BORN with;
/// this is how one of them is singled out afterwards, which is the only form the override can have,
/// because a pane's id is minted at runtime and no config file can name it in advance.
///
/// # Why it prints what the KERNEL holds, not what you typed
///
/// A ceiling on a host whose `memory` controller was never delegated is a number that goes nowhere.
/// Echoing the request back would be sprag agreeing with itself about a setting that is not in
/// force — so the answer is re-read out of the pane's own cgroup, and a row that says
/// `(no memory controller)` is telling a person to go and change their delegation rather than their
/// command line.
///
/// **A share is a WEIGHT, not a cap and not a ratio.** Nothing here renders it as a predicted share
/// of the machine: a nominal 10:100 was measured at 18:82, and a cgroup weighted 10 took all eight
/// cores it was offered once its sibling went idle. `sprag resources` is where the number that
/// actually happened lives.
///
/// `0` removes a ceiling, which is the same spelling `pane-memory-limit = 0` already has in the
/// config file.
fn grant(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "grant")?;
    let mut rest = rest.into_iter();
    let asked = required_pane(rest.next(), "grant")?;
    let mut action_args = serde_json::Map::new();
    while let Some(flag) = rest.next() {
        let key = match flag.as_str() {
            "--share" => "share",
            "--memory" => "memory",
            "--processes" => "processes",
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("grant: unexpected argument {other:?}"),
                ));
            }
        };
        let value = rest.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("grant: {flag} needs a number"),
            )
        })?;
        let number: u64 = value.parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("grant: {flag} takes a number, not {value:?}"),
            )
        })?;
        // Refused here rather than silently taken, because the second one is the one the person
        // meant and a command that quietly used the first would be wrong in the direction nobody
        // checks.
        if action_args.insert(key.to_owned(), json!(number)).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("grant: {flag} given twice"),
            ));
        }
    }
    if action_args.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "grant needs at least one of --share, --memory or --processes",
        ));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_pane(&mut conn, session.as_deref(), &asked, "grant")?;
    action_args.insert("pane".to_owned(), json!(site.id));
    let answer: Value = invoke_action(
        &mut conn,
        site_invoke(
            session.as_deref(),
            &site,
            mux_action_path(GRANT_PANE_ACTION),
            Value::Object(action_args),
        ),
    )
    // The daemon knows WHICH of these it refused and cannot say so — `InvokeError::Rejected` carries
    // no payload (upstream PINION-PR82) — so the sentence lists the causes rather than guessing one.
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::other(format!(
                "grant: pane {} is gone, --share is outside 1..=10000, or this host enforces \
                 nothing",
                site.id
            ))
        } else {
            error
        }
    })?;
    let granted: sprag_terminal::Granted = serde_json::from_value(answer)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    println!(
        "{}: weight {}  memory {}  processes {}",
        site.id,
        weight(granted.share),
        ceiling(granted.memory, footprint_ceiling),
        ceiling(granted.processes, count_ceiling),
    );
    Ok(())
}

/// A ceiling ALONE, for the one surface whose whole subject is the ceiling.
///
/// [`of`] prints a ceiling beside a usage and stays silent when there is none, because the usage
/// column has already said what needed saying. Here there is no usage to hide behind: a person who
/// ran `grant` and got a blank would not know whether the ceiling was removed or never took.
fn ceiling(ceiling: Ceiling, spell: fn(u64) -> String) -> String {
    match ceiling {
        Ceiling::At(most) => spell(most),
        Ceiling::Uncapped => "uncapped".to_owned(),
        Ceiling::NoController => "(no controller)".to_owned(),
    }
}

/// A pane's CPU as a person reads it — cores to two decimals, with the window it covers.
///
/// The window is printed rather than assumed because it is what makes the number a claim: four cores
/// over 40 ms is a build starting and four cores over a minute is a build running away.
fn held(cpu: Cpu) -> String {
    match cpu {
        Cpu::Held {
            millicores,
            over_ms,
        } => format!(
            "{}.{:02} cores over {}.{:01}s",
            millicores / 1000,
            (millicores % 1000) / 10,
            over_ms / 1000,
            (over_ms % 1000) / 100,
        ),
        Cpu::Settling => "(no rate yet)".to_owned(),
    }
}

/// How much of the last ten seconds the pane spent runnable and not running.
fn waited(waiting: Waiting) -> String {
    match waiting {
        Waiting::Measured { avg10, .. } => avg10.to_string(),
        // Not "0%": this kernel keeps no pressure accounting, and a pane that may have waited for
        // everything would be printed as one that never waited at all.
        Waiting::NotAccounted => "(unaccounted)".to_owned(),
    }
}

/// A pane's memory, in the largest unit that keeps it readable.
fn footprint(memory: Counted) -> String {
    match memory {
        Counted::Now(bytes) if bytes >= 1 << 30 => {
            format!("{}.{} GiB", bytes >> 30, (bytes >> 20) % 1024 * 10 / 1024)
        }
        Counted::Now(bytes) if bytes >= 1 << 20 => format!("{} MiB", bytes >> 20),
        Counted::Now(bytes) => format!("{bytes} B"),
        // The controller never reached this pane, so there is no number — which a `0 B` would
        // report as a pane using no memory at all.
        Counted::NoController => "(no memory controller)".to_owned(),
    }
}

/// WHICH BUILD each end of this connection is, as [`doctor`]'s opening lines — one line when they
/// agree, three when they do not, and a different three when the daemon cannot say.
///
/// # ⚠⚠⚠⚠⚠ Why a skew is a FINDING and not a footnote (register item 438)
///
/// A daemon outlives its clients by design: `sprag` is rebuilt every time anybody touches the tree
/// and the daemon is whatever was running when it was last started. So the ordinary state after a
/// day's work is a daemon running code the tree has already replaced — and NOTHING said so. What
/// that cost, measured 2026-08-18: a loop's whole walk was read as evidence about two commits the
/// daemon driving it did not contain, and the walk of a daemon that carried them is the same text.
/// The only probe that answered was `grep` over `/proc/<pid>/exe`.
///
/// It is deliberately not a refusal. `WIRE_PROTOCOL` owns refusal, and it is right to: a shape
/// neither end can parse must stop, where a behaviour skew is a fact a reader acts on. Refusing
/// here would make every rebuild a forced restart of a daemon holding somebody's panes.
///
/// # ⚠⚠⚠⚠ The third case is the one this function exists to keep honest
///
/// A daemon that answers no build is NOT a daemon that matches. The absent key means *it cannot
/// say* — see `sprag_rpc::BUILD_FIELD`, whose whole argument for needing no protocol bump is that
/// nobody reads its absence as a promise. Printing "agree" there would break that argument and
/// earn the number, which is why the three cases are three and not two.
///
/// Pure, and takes the daemon's answer rather than a connection, so the sentences are gateable
/// without a live daemon — including the case a live daemon cannot be made to produce, which is an
/// old one that does not carry the key.
fn build_report(daemon: Option<&str>) -> String {
    let client = sprag_host::wire::BUILD;
    match daemon {
        Some(build) if build == client => format!("build {client} (daemon and client agree)\n"),
        Some(build) => format!(
            "build {client} (this client)\nbuild {build} (the running daemon)\n\
             the daemon is running other code than this tree built; restart it to promote \
             (`sprag kill-server`, then start it again) — until then a run's walk is evidence \
             about the daemon's build, not about yours\n"
        ),
        None => format!(
            "build {client} (this client)\nbuild unknown (the running daemon does not say)\n\
             a daemon predating this answer cannot be dated; an absent build is not a matching \
             one\n"
        ),
    }
}

/// **WHICH BUILD EVERY ATTACHED WINDOW SAID IT IS**, held against the daemon's own — [`doctor`]'s
/// third build section, and the one about the process a person is actually looking at.
///
/// # ⚠⚠⚠⚠⚠ The window was the one companion nothing could date (register item 463)
///
/// This daemon RESOLVES two of its companions: the hook and the MCP server are the sibling of the
/// running executable, so a daemon cannot hand its agents a reporter from another build. **The GUI
/// is outside that structure and always has been** — it is started by hand from wherever somebody
/// points, and this repository's own promotion procedure copies the daemon into one directory and
/// then runs `target/debug/sprag-gui`. So the skew is the ORDINARY state here rather than an exotic
/// one, and the owner raised it the moment it bit: an experimental window driving a daemon built
/// from other code, with nothing anywhere able to say so.
///
/// ⚠⚠⚠ **IT TELLS, IT NEVER REFUSES**, and that is the same ruling `sprag_rpc::BUILD_FIELD` states
/// for the daemon's own build: `WIRE_PROTOCOL` owns refusal because a shape neither end can parse
/// must stop, where a behaviour skew is a fact a reader acts on. A GUI refused at the door over a
/// build difference would throw a person out of the windows they are working in, every rebuild.
///
/// # ⚠⚠⚠⚠ The counts are printed even when nothing is wrong, and that is the substance
///
/// A report that printed only the odd ones leaves a reader unable to tell *every window was checked
/// and matched* from *nobody looked* — and this surface is read exactly when somebody already
/// suspects the answer. So the summary states how many were compared and how each of them came out;
/// silence here is earned rather than assumed.
///
/// # Four answers, and the fourth belongs to the SET rather than to any client
///
/// The comparison is `sprag_host::wire::reporter_image`'s, shared with the hook's sentence one verb
/// over so the two mouths cannot come to count differently. Three of its arms are per-client. The
/// fourth — a daemon that cannot say its OWN build — is a property of the daemon every row is being
/// held against, so it is said once, for however many clients it swallowed. Rendering it per row
/// would repeat one fact N times and read as N problems.
///
/// Pure, and takes the two facts rather than a connection, so every answer is gateable — including
/// the daemon-silent one, which no live daemon in this tree can be made to produce.
fn attached_build_report(daemon: Option<&str>, clients: &[(String, Option<String>)]) -> String {
    if clients.is_empty() {
        return "no client is attached, so no window's build was compared\n".to_owned();
    }
    let (mut same, mut uncomparable) = (0_usize, 0_usize);
    let mut findings = String::new();
    let (mut other, mut unsaid) = (0_usize, 0_usize);
    for (client, build) in clients {
        match sprag_host::wire::reporter_image(build.as_deref(), daemon) {
            sprag_host::wire::ReporterImage::SameImage { .. } => same += 1,
            // ⚠ THE ONE A READER MUST ACT ON: the window on the screen is drawing from code this
            // daemon has never run. BOTH builds are named — one of them alone tells a reader
            // nothing about which is which.
            sprag_host::wire::ReporterImage::OtherImage { reporter, daemon } => {
                other += 1;
                findings.push_str(&format!(
                    "⚠ THE WINDOW {client:?} IS NOT THIS DAEMON'S IMAGE: it is build {reporter} \
                     and this daemon is build {daemon}. What it draws, and every key it sends, is \
                     that build's behaviour — close it and start the `sprag-gui` beside this \
                     daemon, or restart the daemon to promote the tree the window came from\n"
                ));
            }
            // ⚠⚠ Counted for the SET, said once below: a daemon that cannot say its own build
            // makes every comparison impossible for the same single reason.
            sprag_host::wire::ReporterImage::DaemonSilent { .. } => uncomparable += 1,
            // ⚠⚠⚠ AND THE ARM A TIDY EDIT FOLDS INTO THE FIRST. Every client older than
            // `sprag_rpc::CLIENT_BUILD_PARAM` says exactly nothing, and reading that as agreement
            // would make the commonest case look like the safe one.
            sprag_host::wire::ReporterImage::ReporterSilent => {
                unsaid += 1;
                findings.push_str(&format!(
                    "{client:?} does not say which build it is, which is not the same as saying it \
                     matches — a client older than this key answers exactly this\n"
                ));
            }
        }
    }
    let mut report = format!(
        "{} attached client(s): {same} on the daemon's build, {other} on other code, {unsaid} did \
         not say\n",
        clients.len(),
    );
    if uncomparable > 0 {
        report.push_str(&format!(
            "{uncomparable} of them stated a build and this daemon does not say which build IT is, \
             so those could not be compared — an absent build is not a matching one\n"
        ));
    }
    report.push_str(&findings);
    report
}

/// **WHICH WINDOWS FOLLOW NOBODY** — register item 482, and the sentence that was missing when a
/// terminal stopped resizing and its owner concluded the code had been hardcoded.
///
/// # ⚠⚠⚠⚠⚠ A pin is a moment; it is recorded as a policy and announced by nothing
///
/// `resize-window` pins a window to a size and flips `window-size` to `manual`, a policy that **does
/// not read the attached clients at all**. The value is written to the config file, so one act
/// outlives the window, the daemon and every later session — and until this report, no surface
/// anywhere said a window was pinned. It simply stopped following, which is indistinguishable from
/// breakage: moving the terminal to another monitor, resizing the OS window and dragging a splitter
/// all change nothing, because the window is doing exactly what it was told.
///
/// ⚠⚠⚠ **THE REMEDY IS NAMED, WHICH IS WHY THIS IS A REPORT AND NOT A REFUSAL.** Pinning is a real
/// thing an operator asks for and `latest` is already the default, so nothing here second-guesses
/// the act — item 463 settled this shape for the sibling fact: *impossible by construction where it
/// can be, visible by REPORT where it cannot.*
///
/// ⚠⚠ Pure, and takes the rows rather than a connection, so every answer is gateable — including
/// the pinned one, which needs a daemon somebody has pinned a window on.
fn pinned_window_report(sessions: &[(String, usize, usize)]) -> String {
    let pinned: Vec<&(String, usize, usize)> = sessions
        .iter()
        .filter(|(_, _, pinned)| *pinned > 0)
        .collect();
    if pinned.is_empty() {
        // ⚠ SAID OUT LOUD rather than left silent, `attached_build_report`'s rule: a reader must be
        // able to tell *every window follows its clients* from *nobody looked at this*.
        return "no window is pinned, so every window follows the clients attached to it\n"
            .to_owned();
    }
    let mut report = String::new();
    for (name, windows, count) in pinned {
        report.push_str(&format!(
            "⚠ {count} of session {name:?}'s {windows} window(s) FOLLOW NO CLIENT: somebody pinned \
             a size, so resizing the terminal, moving it to another monitor and dragging a splitter \
             all change nothing — `sprag resize-window -u -t {name}` hands the window back\n"
        ));
    }
    report
}

/// `doctor`: what is WRONG with the machine the panes run on.
///
/// # Why it prints the healthy rows too, and why every row carries a number
///
/// A report that printed only the faults would leave a person unable to tell *this was fine* from
/// *nobody looked*, and the design this implements makes that distinction the whole point: seven
/// causes were found in the investigation behind it and only one belonged to the multiplexer, so
/// most of what a person needs is confidence that the other six were checked. Each row therefore
/// prints its verdict, what it measured, and — for anything not clean — the source it read, the
/// criterion it was judged by and what a person could do. The command NEVER does that last thing:
/// detection and evidence are automated, the prescription is typed by a human.
///
/// # Why it pauses
///
/// One check cannot be answered by a snapshot. A cumulative counter says a neighbour used CPU at
/// some point since boot, and the question is whether it is taking it now — so the daemon samples
/// the levels above its own subtree twice, half a second apart, and every rate states the window it
/// covers.
fn doctor(args: Vec<String>) -> io::Result<()> {
    if let Some(extra) = args.first() {
        return Err(bad_input(&format!(
            "doctor: unexpected argument {extra:?} (it takes none — a machine is not divided by \
             session)"
        )));
    }
    let mut conn = connect()?;
    print!("{}", build_report(conn.daemon_build()));
    // ⚠⚠⚠⚠⚠ AND THE BUILD OF EVERY WINDOW SOMEBODY IS LOOKING AT — register item 463. The pair
    // above is THIS process against the daemon, which says nothing about the `sprag-gui` on the
    // screen: that one is started by hand from wherever a person points, so it is the companion
    // this daemon does not resolve and the only one that could differ unnoticed. The daemon holds
    // every client's stated build beside its own, so it is the party that can be asked.
    //
    // ⚠ Read BEFORE the pause below, deliberately: the machine checks sample twice half a second
    // apart, and a build skew is the fact most likely to explain everything printed after it.
    let daemon_build = conn.daemon_build().map(str::to_owned);
    let attached = query_slot(&mut conn, json!({ "path": mux_action_path(CLIENTS_SLOT) }))?;
    let attached: Vec<(String, Option<String>)> = attached
        .as_array()
        .into_iter()
        .flatten()
        .map(|client| {
            (
                client["client"].as_str().unwrap_or("?").to_owned(),
                // Absent for a client that did not say, which is the `None` the renderer must be
                // handed rather than a fabricated word — the whole key's rule.
                client["build"].as_str().map(str::to_owned),
            )
        })
        .collect();
    print!(
        "{}",
        attached_build_report(daemon_build.as_deref(), &attached)
    );
    // ⚠⚠⚠⚠⚠ AND WHICH WINDOWS FOLLOW NOBODY — register item 482. Read BESIDE the build comparison
    // and for its reason: both are facts about the window a person is looking at that the machine
    // checks below cannot reach, and a pinned window explains a terminal that looks broken more
    // often than anything printed after it. ⚠ Registry-WIDE like `clients` above, so this stays
    // `doctor`'s own contract — *a machine is not divided by session*.
    let sessions = query_slot(&mut conn, json!({ "path": mux_action_path(SESSIONS_SLOT) }))?;
    let sessions: Vec<(String, usize, usize)> = sessions
        .as_array()
        .into_iter()
        .flatten()
        .map(|session| {
            (
                session["name"].as_str().unwrap_or("?").to_owned(),
                session["windows"].as_u64().unwrap_or(0) as usize,
                // ⚠ ABSENT IS ZERO and that is the honest reading, not a fabricated one: the key is
                // skipped for every unpinned session, and a daemon too old to publish it is one
                // where the fact could not have been established either.
                session["pinned"].as_u64().unwrap_or(0) as usize,
            )
        })
        .collect();
    print!("{}", pinned_window_report(&sessions));
    let answer = query_slot(
        &mut conn,
        json!({ "path": mux_action_path(&doctor_over(DOCTOR_WINDOW_MS)) }),
    )?;
    let report = serde_json::from_value::<Diagnosis>(answer)
        .map_err(|error| bad_input(&format!("doctor: the host's answer did not parse: {error}")))?;
    let degraded = report.degraded().count();
    let blind = report
        .findings
        .iter()
        .filter(|finding| matches!(finding.verdict, Verdict::Blind(_)))
        .count();
    println!(
        "{} checks: {degraded} degraded, {blind} not measurable, {} clean",
        report.findings.len(),
        report.findings.len() - degraded - blind,
    );
    for finding in &report.findings {
        let entry = finding.check.entry();
        println!();
        println!("{} {}", verdict_mark(finding.verdict), entry.name);
        for row in finding.evidence.rows() {
            println!("    {}: {}", row.of, row.is);
        }
        // The source, the bar and the remedy only where they are load-bearing. On a clean row they
        // would be four lines of advice about something that is not happening, and eleven of those
        // is a report nobody reads to the end.
        if finding.verdict == Verdict::Degraded {
            println!("    asks: {}", entry.asks);
            println!("    read: {}", entry.source);
            println!("    flagged when: {}", entry.criterion);
            println!("    you could: {}", entry.remedy);
        }
    }
    Ok(())
}

/// HOW TO CALL THE DAEMON'S VERBS, asked of the daemon — `sprag show-grammar [VERB] [--pane]`.
///
/// # Why this is a verb and not a document
///
/// The wire has published its call grammar since R352 and the only way to read it was to write a raw
/// JSON-RPC probe. That is a surface working and a feature nobody can use, so this is the door: one
/// `scene/query` at `action_grammar`, printed.
///
/// ⚠ **IT ASKS THE DAEMON, and that is the whole discriminator against the closest rival.** herdr
/// ships `herdr api schema`, which prints a JSON Schema a test wrote into `docs/` and the binary
/// `include_str!`'d at build time (`src/cli/api.rs:1` at `9a4ce5e1`) — so it describes the build the
/// CLI came from, and none of its ninety-one methods returns it, which means a client speaking the
/// socket cannot ask the daemon it is connected to. This prints what the RUNNING daemon answered: an
/// operator debugging a version-skewed daemon sees ITS grammar, not this binary's idea of it.
///
/// # What it prints
///
/// One line per argument, grouped by verb and by FORM, because a verb with two forms is a choice a
/// caller makes and not a union of keys:
///
/// ```text
/// key
///   form scalar
///     key            string   required
///   form object
///     key            string   required
///     state          string   optional  one of: down, up
/// ```
///
/// `--pane` reads a PANE's surface instead of the multiplexer's — the six input verbs, which hang off
/// a per-pane address and are therefore a different table (see `ACTION_GRAMMAR_SLOT`). Any pane will
/// do because every pane's input surface serves the same grammar, so it takes the caller's active one
/// rather than making them name one.
fn show_grammar(args: Vec<String>) -> io::Result<()> {
    let (mut only, mut surface, mut session) = (None, GrammarSurface::Mux, None);
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            flag if GrammarSurface::of_flag(flag).is_some() => {
                surface = GrammarSurface::of_flag(flag).expect("just matched");
            }
            "-t" | "--session" => {
                session = Some(
                    rest.next()
                        .ok_or_else(|| bad_input("show-grammar: -t takes a session name"))?,
                );
            }
            other if other.starts_with('-') => {
                return Err(bad_input(&format!(
                    "show-grammar: unknown option {other:?} (it takes [VERB] and one of {})",
                    GrammarSurface::flags().join(", "),
                )));
            }
            verb if only.is_none() => only = Some(verb.to_owned()),
            extra => {
                return Err(bad_input(&format!(
                    "show-grammar: unexpected argument {extra:?} (one verb at a time)"
                )));
            }
        }
    }

    let mut conn = connect()?;
    // THE ADDRESS THE ANSWER DESCRIBES. A pane's grammar is served by the pane, so asking the
    // multiplexer for it would be asking the wrong surface — which is exactly the confusion a single
    // global table would have created.
    let path = match surface {
        GrammarSurface::Mux => mux_action_path(ACTION_GRAMMAR_SLOT),
        GrammarSurface::Plugins => sprag_host::wire::plugins_path(ACTION_GRAMMAR_SLOT),
        GrammarSurface::PaneInput => {
            // ANY pane: every pane's input surface serves the same verbs, so the first one this
            // session holds answers the question, and making the caller name one would suggest the
            // answer differs per pane. A session with no panes cannot answer it at all, and says so.
            let pane = *pane_ids(&mut conn, session.as_deref())?
                .first()
                .ok_or_else(|| {
                    bad_input(
                        "show-grammar --pane: this session has no pane, and a pane's input grammar \
                         is served by a pane",
                    )
                })?;
            pane_input_path(pane, ACTION_GRAMMAR_SLOT)
        }
    };
    let answer = query_slot(&mut conn, scoped_params(session.as_deref(), path))?;
    let verbs = answer
        .as_object()
        .ok_or_else(|| bad_input("show-grammar: the host's answer was not an object of verbs"))?;

    if let Some(wanted) = &only
        && !verbs.contains_key(wanted)
    {
        {
            // NAMES WHAT THERE IS, because the commonest reason to be here is not knowing the verb's
            // spelling — and a bare "unknown" would send the reader back to ask the same question.
            let mut known: Vec<&str> = verbs.keys().map(String::as_str).collect();
            known.sort_unstable();
            return Err(bad_input(&format!(
                "show-grammar: this daemon's {} surface publishes no grammar for {wanted:?}. It \
                 publishes: {}. Another surface may serve that verb — {} name the others.",
                surface.tag(),
                known.join(", "),
                GrammarSurface::flags().join(" and "),
            )));
        }
    }

    // ⚠ THROUGH THE ONE READER OF A PUBLISHED GRAMMAR, and it was a hand-rolled JSON walk here
    // until R355. Two readers of one answer is the shape this whole surface exists to remove, and
    // the second one had already gone wrong: it printed a nested argument's TYPE and never its
    // fields, so `guardrails object optional` was the entire truth an operator got about the
    // ceilings inside it — the affirmative silence the nesting was added to end, printed by the
    // verb whose job is to end it. (Two ceilings then, three now: the set it would have hidden has
    // grown since, which is what an undescribed object costs over time.)
    let surface =
        sprag_rpc::read_surface(&answer).map_err(|error| bad_input(&format!("{error}")))?;
    let mut surface = surface;
    surface.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, forms) in &surface {
        if only.as_ref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        println!("{name}");
        for form in forms {
            println!("  form {}", form.form.wire_str());
            for arg in &form.args {
                print_grammar_arg(arg, 4);
            }
            if form.args.is_empty() {
                println!("    (no arguments)");
            }
        }
    }
    Ok(())
}

/// One published argument, indented, with the arguments INSIDE it under it.
///
/// Recursive because [`sprag_rpc::PublishedArg::fields`] is: a nested value's grammar is a grammar,
/// which is the answer R355 gave to *"is a nested argument a recursive grammar or an address of its
/// own?"*. A printer that stopped at the top level would be publishing the same silence the
/// declaration stopped publishing.
fn print_grammar_arg(arg: &sprag_rpc::PublishedArg, indent: usize) {
    let need = if arg.optional { "optional" } else { "required" };
    let words = arg.words.as_ref().map_or_else(String::new, |words| {
        format!("  one of: {}", words.join(", "))
    });
    let pad = " ".repeat(indent);
    let width = 14_usize.saturating_sub(indent.saturating_sub(4));
    println!("{pad}{:<width$} {:<8} {need}{words}", arg.name, arg.ty);
    // ⚠ A LIST OF OBJECTS says whose fields these are. Indented under `array`, the two shapes an
    // array can have — a list of strings and a list of objects — print identically, so a reader
    // cannot tell "send these keys once" from "send this many times, each with these keys". The
    // grammar knows; the printer was the only thing not saying it.
    if arg.is_a_list_of_objects() {
        println!("{pad}  each entry:");
    }
    for field in &arg.fields {
        print_grammar_arg(field, indent + 2);
    }
}

/// The CLI-only flag on `orchestrate` that PARKS until the run ends.
///
/// Named here because it is the one word in that verb's command line that is not an argument of the
/// wire action, and [`orchestrate`] refuses to run at all if the daemon ever publishes an argument
/// spelled the same — see there for why that is a refusal and not a rename.
const WAIT_FLAG: &str = "wait";

/// The CLI-only flag on `orchestrate` that answers whether THIS DAEMON TAKES THIS CALL, and starts
/// nothing.
///
/// # 🎯 The question a caller could not ask without paying for the answer — register item 855
///
/// `orchestrate` builds its call from the grammar the daemon publishes, so a key that daemon does
/// not carry is refused BY NAME (`--loop_kind is not an argument of this call`). That refusal is
/// exact and it is late: it arrives from the launch itself, so a caller who wanted to know first
/// had nowhere to ask. What a launcher did instead was WRITE THE ANSWER DOWN — the debt loop's
/// `launch.sh` carried a comment saying which daemon commit its call needed — and on 2026-09-03
/// that constant was a commit behind the truth while every guard around it stayed green. A
/// measurement kept in a second place goes stale silently; this is the first place.
///
/// # ⛔⛔⛔⛔⛔ AND IT ANSWERED ONLY THE FILL, WHILE SAYING IT WAS THE CHECK — register item 873
///
/// This doc used to end *"what it answers is exactly the fill, and no more"*, which was true and
/// was the defect. Answering the fill means answering with the CLIENT, and a client cannot read
/// this repository's loop-kind document — so a call the check reported taken was refused at launch
/// with *"this run named neither `agent` nor `ready_when`, and this repository's loop-kind document
/// authors no barrier either"* (wz `f8`, 2026-09-03). **A green check followed by a red launch is
/// worse than no check**, because a launcher branches on it.
///
/// ⇒ The flag is now SENT, on the very call that would launch
/// ([`PluginGrammar::DRY_RUN`](sprag_host::wire::PluginGrammar)), and the daemon stops one line
/// above the spawn. There is no longer a set of checks the dry run runs and the launch does not, or
/// the other way about, because there is no longer a second set — the two are one door.
///
/// ⚠ What is STILL unanswered, and the verdict says so: **whether the run converges.** That is not
/// a fact about the call and no door can hold it.
///
/// ⚠⚠ The residue, stated rather than hidden: this now needs a ROUND TRIP where it used to need
/// none, and against a daemon predating the key it is refused by name like any unknown word. Both
/// are the price of the answer being the daemon's, which is the only way it can be right.
///
/// # ⚠⚠⚠⚠⚠ AND IT IS NO LONGER ONE OF [`OWN_FLAGS`], WHICH IS THE POINT
///
/// It was this command's own word while this command answered it. Now the daemon publishes
/// `dry_run` as an argument of every run form, so claiming the same word here would be the exact
/// collision [`own_flag_collision`] refuses — and that guard is what CAUGHT this change, failing
/// the new gate before the rename was made. The word is the daemon's; this constant is only how
/// this file spells it in its own sentences, and the value is read back from the BUILT CALL
/// ([`sprag_host::plugins::RUN_DRY_RUN_KEY`]) so what the verdict describes and what the daemon
/// was asked cannot come apart.
const DRY_RUN_FLAG: &str = "dry-run";

/// One word on `orchestrate`'s command line that belongs to THIS COMMAND rather than to the daemon.
///
/// # ⚠⚠ Everything a caller can learn about such a flag is HERE, and that is the point
///
/// `orchestrate`'s every other word is read off the published grammar, so these are the only ones
/// this binary has to describe itself. Register item 864: when item 855 added the second of them,
/// the collision check walked a table while `--help` spelled the two by hand — so the table could
/// grow and the usage would not, which is item 855's own defect (a fact kept in a second place)
/// reappearing inside its repair. The fields exist so that every consumer reads the same row.
struct OwnFlag {
    /// The flag as typed, without the leading dashes.
    name: &'static str,
    /// What it does, for `--help`. One line, no trailing full stop — the printer adds none either.
    does: &'static str,
    /// What a caller does INSTEAD when the daemon publishes an argument of this name, for the
    /// refusal — see [`own_flag_collision`].
    instead: &'static str,
}

/// The words on `orchestrate`'s command line that belong to THIS COMMAND rather than to the daemon.
///
/// One table rather than one check per flag, because the property is about the NAMESPACE and not
/// about any of them: every other word there is read off the published grammar, so a daemon that
/// ever publishes an argument spelled like one of these makes two meanings out of one word.
const OWN_FLAGS: &[OwnFlag] = &[OwnFlag {
    name: WAIT_FLAG,
    does: "parks until the run ends and prints the outcome",
    instead: "parking until a run ends. Start the run without it and read `sprag runs`.",
}];

/// How often [`orchestrate`] `--wait` re-reads the run's state.
///
/// A poll and not a subscription because a run's outcome is a LEVEL: the `runs` slot answers where
/// a run got to whether or not anybody was watching when it got there, which is the same property
/// `agent_state` is a level for. A missed edge costs nothing here.
const RUN_POLL: Duration = Duration::from_millis(120);

/// `orchestrate PLUGIN [--ARG VALUE]… [--wait] [--dry-run]`: start a BOUNDED loop and print its run
/// id — or, with `--dry-run`, say whether this daemon takes the call and start nothing.
///
/// # This verb is the door the product's headline feature did not have
///
/// The README's first line names *"AI↔AI 오케스트레이션 루프"*, and until R355 it was reachable from
/// no mouth of this product: not a CLI verb, not an MCP tool, not a keystroke, not a palette row.
/// The loop itself was built and well built — four plugins, an SCXML driver, an iteration ceiling, a
/// typed cost ceiling that cannot bind bytes with a token budget, a cancel flag, agent-state-aware
/// turn ends — and the only way in was to hand-write
/// `scene/invoke /sprag_plugins/external/run`.
///
/// # ⚠⚠ Its arguments are not written down here, and that is the design
///
/// `run` has FOUR forms, one per plugin, and the daemon publishes all four on `action_grammar`.
/// This verb READS that publication and builds the call from it
/// ([`sprag_rpc::build_call`]), so:
///
/// * there is no second list of argument names in this binary to drift from the daemon's;
/// * `--help` prints the forms the DAEMON serves, not the ones this build was compiled against, so
///   version skew is visible instead of silent;
/// * a plugin added upstream is callable from this shell with no edit here at all.
///
/// That is the first consumer of the published grammar that ACTS on it. `show-grammar` proved a
/// client can ask; this proves the answer is enough to call with — which is the claim the whole
/// surface was built on and which nothing had ever driven.
///
/// The discriminator is POSITIONAL for the same reason it is published: the grammar says which
/// argument chooses a form, so `sprag orchestrate agent …` needs no `--plugin`.
fn orchestrate(args: Vec<String>) -> io::Result<()> {
    let wants_help = args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h");
    let (session, args) = scope_and_rest(args, "orchestrate")?;
    // PRE-FLIGHTED, like every window and pane verb — see [`runs`] for the answer a mistyped
    // session used to get from this family.
    let mut conn = connect_scoped(session.as_deref())?;
    let forms = published_forms(
        &mut conn,
        session.as_deref(),
        sprag_host::plugins::RUN_ACTION,
    )?;
    let (selector, words) = sprag_rpc::selector_of(&forms);

    if wants_help {
        print!("{}", orchestrate_usage(&forms, &selector, OWN_FLAGS));
        return Ok(());
    }

    if let Some(collision) = own_flag_collision(&forms) {
        return Err(bad_input(&collision));
    }

    let mut flags = Vec::new();
    let mut wait = false;
    let mut rest = args.into_iter().peekable();
    // The FIRST bare word is the plugin, under whatever name the publication says chooses a form.
    if let Some(selector) = &selector
        && rest.peek().is_some_and(|arg| !arg.starts_with('-'))
    {
        flags.push(Flag::new(
            selector.clone(),
            rest.next().expect("just peeked"),
        ));
    }
    while let Some(arg) = rest.next() {
        let Some(spelled) = arg.strip_prefix("--") else {
            return Err(bad_input(&format!(
                "orchestrate: {arg:?} is not a flag. Say the plugin first, then --name value."
            )));
        };
        // BOTH SPELLINGS (R350): `--key value` and `--key=value`. The second is not a convenience
        // here — it is the only way to pass a value that begins with a dash, which an argv template
        // (`--endpoint-a=-p`) needs.
        if let Some((name, value)) = spelled.split_once('=') {
            flags.push(Flag::new(name, value));
            continue;
        }
        if sprag_rpc::call::same_name(spelled, WAIT_FLAG) {
            wait = true;
            continue;
        }
        // ⚠⚠⚠ `--dry-run` IS NOT INTERCEPTED HERE ANY MORE — register item 873. It is the daemon's
        // argument now, so it falls through to the bare-flag arm below and is filled and typed like
        // every other published word. Swallowing it here would make this parser the second place
        // that decides what it means, which is the defect this item pays one layer up.
        match rest.peek() {
            Some(value) if !value.starts_with("--") => {
                flags.push(Flag::new(spelled, rest.next().expect("just peeked")));
            }
            // A bare flag. Well-formed only for a bool, and the fill says so in the argument's own
            // terms rather than this parser guessing which it was.
            _ => flags.push(Flag::bare(spelled)),
        }
    }

    // ⚠⚠⚠⚠⚠ A PANE IS RESOLVED HERE, THROUGH THE DOOR EVERY OTHER PANE VERB USES — register item
    // 542. The published grammar declares this argument an `int`, so a NAME reached `build_call` as
    // a type error and was refused before the daemon was ever asked — on the ONE verb whose target
    // a person types least often and remembers least well.
    //
    // ⚠⚠⚠⚠⚠ AND BOTH SPELLINGS GO THE SAME WAY — register item 686, which is item 542's NEXT
    // LINE and was measured on the sentence this comment used to carry. It said a bare number
    // "used to be" read against the current window, and that was false the day it was written: the
    // guard here was `raw.parse::<u64>().is_err()`, so a NUMBER never reached the resolver at all
    // and went out exactly as typed. What is answered per-window is not this resolution but the
    // DAEMON's `require_pane_in`, which reads one window's pane pool — see below.
    //
    // ⚠⚠ MATCHED BY NAME, and the residue is stated rather than hidden: `pipe`'s `src`/`dst` are
    // panes too and are NOT resolved here. The grammar gives nothing to detect pane-ness with —
    // every one of them is published as `int` — so widening this would be a second hardcoded list
    // rather than a rule, and item 542 asks for `--pane`.
    let mut site = None;
    for flag in &mut flags {
        if sprag_rpc::call::same_name(&flag.name, sprag_rpc::PANE_PARAM)
            && let Some(raw) = flag.value.as_deref()
        {
            let resolved = resolve_pane(&mut conn, session.as_deref(), raw, "orchestrate")?;
            flag.value = Some(resolved.id.to_string());
            site = Some(resolved);
        }
    }

    // ⛔⛔⛔⛔⛔ AND WHO IS ASKING — register item 871, the half item 865 could not reach.
    //
    // Item 865 gave the run row a mouth for its asker. This is the door that fills it: measured on
    // the live loop daemon, **190 of 190 runs carried no conversation**, because this binary — the
    // one every loop is launched from — had no way to say who it was. The MCP surface has stamped
    // its own pane since it existed; a shell never could.
    //
    // ⚠⚠ THE VARIABLE IS THE DAEMON'S OWN STAMP. `SPRAG_PANE` is written into a pane's environment
    // by the host that spawned it, so a process reading it is not asserting an identity, it is
    // repeating one it was given — which is exactly what `PluginGrammar::OPENED_BY` calls the key:
    // *"PROVENANCE and not authorisation"*. That doc also said absence means *"a run nobody claims
    // — which is what a person starting one from a shell is"*, and that premise is what item 871
    // measured false: the shells starting these runs are agents' panes, not people's.
    //
    // ⚠ A caller who named their own wins, on `identity_args`' rule: somebody who said which pane
    // asked has said it, and a second answer would be this binary silently overriding theirs.
    if !flags
        .iter()
        .any(|flag| sprag_rpc::call::same_name(&flag.name, sprag_host::plugins::RUN_OPENED_BY_KEY))
        && let Some(mine) = asking_pane(&mut conn, session.as_deref())
    {
        flags.push(Flag::new(
            sprag_host::plugins::RUN_OPENED_BY_KEY,
            mine.to_string(),
        ));
    }

    let call = sprag_rpc::build_call(&forms, &flags).map_err(|error| {
        bad_input(&format!(
            "orchestrate: {error}\n{}",
            usage_for(&forms, &flags, &selector, &words),
        ))
    })?;

    // ⚠⚠⚠⚠⚠ THE CHECK IS NOW A REQUEST, AND THIS IS WHERE THIS COMMAND LEARNS IT WAS ASKED FOR —
    // register item 873. `--dry-run` used to be answered right here without anything being sent,
    // which is precisely why it could only run the checks a CLIENT can run.
    //
    // ⚠⚠ Read off the BUILT CALL and not off a bool this parser set, so the fact this command
    // renders and the fact the daemon acts on are ONE READING of one word. A local flag would be a
    // second answer, and `--dry-run=false` is the input that tells them apart: the fill types it,
    // and a parser counting occurrences would have called it a dry run.
    let dry_run = call
        .get(sprag_host::plugins::RUN_DRY_RUN_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    // ⚠⚠ THE TWO CONTRADICT EACH OTHER, AND THAT IS SAID RATHER THAN RESOLVED. `--wait` parks until
    // a run ends and `--dry-run` starts none, so a command line carrying both has no reading in
    // which each word means what it says. Silently letting one win is the shape item 852 was filed
    // on — a caller's instruction dropped with nothing printed.
    //
    // ⚠ Asked AFTER the fill since item 873, because the fill is now what says whether this is a
    // dry run at all.
    if wait && dry_run {
        return Err(bad_input(&format!(
            "orchestrate: --{WAIT_FLAG} parks until a run ends and --{DRY_RUN_FLAG} starts none, \
             so the two cannot both be meant. Check the call with --{DRY_RUN_FLAG}, then launch it."
        )));
    }
    // ⚠⚠⚠⚠⚠ AND THE REQUEST SAYS WHICH WINDOW THE PANE WAS FOUND IN — register item 686, the
    // half item 542 left standing. Resolving session-wide answers WHICH pane; it does not carry
    // the answer to the daemon. `require_pane_in` is `PluginWorld::has_pane` — ONE window's pane
    // pool — so a request that names no window is read against the CURRENT one, and a pane
    // resolved correctly one window over came back `no pane N in this workspace`. Called by NAME,
    // it refused by NUMBER: that mismatch is the whole diagnosis, because it says the resolver had
    // already done its job and the sentence came from a mouth one layer in.
    //
    // ⚠⚠ [`site_invoke`] rather than a window flag spelled here, because that is the door every
    // other pane-addressed invoke on this binary goes through, and its doc carries the reason
    // sending the window ALWAYS is right: which actions consult it is the daemon's rule, and a
    // client that remembered which is which would be keeping a second copy of it.
    //
    // ⚠ A run with no `--pane` at all (a plugin form that takes none) has no site and keeps the
    // scope-only shape — `None` is "not narrowed" here exactly as it is on `PaneSite::window`.
    let path = sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION);
    let request = match &site {
        Some(site) => site_invoke(session.as_deref(), site, path, call),
        None => scoped_call(session.as_deref(), path, call),
    };
    let answer = invoke_action(&mut conn, request)?;
    // 🎯 THE ANSWER WITHOUT THE ACT — register item 855, reached the long way since item 873.
    // Getting here at all IS the verdict: the daemon ran every check a launch runs and refused
    // nothing, and the only thing it did not do is spawn. That is why the caveat below is one
    // clause shorter than it was — *whether the daemon accepts the run* is no longer unanswered,
    // because this line is the daemon having accepted it.
    //
    // ⚠ The refusal road needs nothing here: `invoke_action` carries the daemon's own sentence up,
    // so a dry run and a launch are refused in the SAME words. That identity is the gate's subject
    // (`a_dry_run_refuses_everything_a_launch_does`) and it is a property of there being one door,
    // not of anything printed here.
    if dry_run {
        println!("orchestrate --{DRY_RUN_FLAG}: this daemon TAKES this call. No run was started.");
        println!("{}", usage_for(&forms, &flags, &selector, &words));
        println!(
            "  Answered: the daemon ran every check a launch runs — the form's own names, \
             required arguments and types, and everything only it can judge (its plugin, its \
             panes, and what this repository's loop-kind document authors). NOT answered: whether \
             the run converges."
        );
        return Ok(());
    }
    let id = answer
        .as_u64()
        .ok_or_else(|| bad_input("orchestrate: the daemon's answer was not a run id"))?;
    println!("run {id} started");
    if wait {
        let finished = wait_for_run(&mut conn, session.as_deref(), id)?;
        print!("{}", render_run(&finished));
    } else {
        println!("  sprag runs           to see how it ends");
        println!("  sprag cancel-run {id}   to stop it");
    }
    Ok(())
}

/// `answer-pane PANE --asked TEXT --answer TEXT`: answer the question a pane's agent is asking.
///
/// # ⚠⚠⚠ The verb `sprag agent` had no counterpart to
///
/// `sprag agent 3` can say a pane is `blocked` and — since R367 — print the dialog it is showing,
/// option by option, marking the one a bare Enter would take. The only thing a person could then do
/// about it was `sprag send-keys 3 2 Enter`, which is two raw keystrokes at a menu: the number is
/// read off a screen that may have re-rendered, the Enter lands wherever the peer has got to, and
/// nothing checks that either was taken.
///
/// This verb is the same act with the evidence attached. It names the question and the option **in
/// the agent's own words**, the daemon re-reads the screen at the moment it answers, and the
/// keystrokes it sends are the ones the peer's own marker justifies — see
/// [`sprag_plugin::Consent`] and [`sprag_plugin::Taken`].
///
/// # ⚠⚠ It WAITS, always, and there is no `--wait` flag
///
/// `orchestrate` returns a run id because a loop outlives the call that started it. An answer does
/// not: it is over in a keystroke and the peer's reaction to it, so a caller that got an id back
/// would have to poll to learn the one thing they asked. The run underneath is bounded by its own
/// guardrails exactly as every other run is — this waits for it, it does not invent a second bound.
///
/// # Errors
///
/// A malformed command line, no daemon, or a refusal from the plugin surface — a pane it does not
/// hold, or a consent it will not read.
fn answer_pane(args: Vec<String>) -> io::Result<()> {
    const USAGE: &str = "answer-pane PANE --asked TEXT --answer TEXT [-t SESSION]\n  \
         --asked   text the QUESTION must carry, so a `Yes` for one dialog cannot answer another\n  \
         --answer  text the OPTION must carry; it must name exactly one, or nothing is typed";
    let (session, args) = scope_and_rest(args, "answer-pane")?;
    let mut pane = None;
    let mut asked = None;
    let mut answer = None;
    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        // BOTH SPELLINGS, `orchestrate`'s rule (R350): `--asked=…` is the only way to pass a needle
        // that begins with a dash, and an agent's options frequently do.
        let (name, inline) = match arg.strip_prefix("--") {
            Some(spelled) => match spelled.split_once('=') {
                Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
                None => (spelled.to_owned(), None),
            },
            None if pane.is_none() => {
                pane = Some(arg);
                continue;
            }
            None => {
                return Err(bad_input(&format!(
                    "answer-pane: unexpected argument {arg:?}\n{USAGE}"
                )));
            }
        };
        let slot = match name.as_str() {
            n if sprag_rpc::call::same_name(n, sprag_host::plugins::CONSENT_ASKED_KEY) => {
                &mut asked
            }
            n if sprag_rpc::call::same_name(n, sprag_host::plugins::CONSENT_ANSWER_KEY) => {
                &mut answer
            }
            _ => {
                return Err(bad_input(&format!(
                    "answer-pane: unknown flag --{name}\n{USAGE}"
                )));
            }
        };
        let value = match inline {
            Some(value) => value,
            None => rest.next().ok_or_else(|| {
                bad_input(&format!("answer-pane: --{name} needs its text\n{USAGE}"))
            })?,
        };
        *slot = Some(value);
    }
    let pane = pane.ok_or_else(|| bad_input(&format!("answer-pane: which pane?\n{USAGE}")))?;
    // ⚠ REFUSED HERE AND AGAIN AT THE DAEMON, and neither is redundant. This one is the shell's
    // usage; the daemon's is the wire's grammar, which a client that is not this binary also meets.
    let asked = asked.ok_or_else(|| {
        bad_input(&format!(
            "answer-pane: --asked names WHICH QUESTION this answers. Without it a `Yes` written \
             for one dialog would answer whatever the pane happens to be showing.\n{USAGE}"
        ))
    })?;
    let answer = answer.ok_or_else(|| {
        bad_input(&format!(
            "answer-pane: --answer names WHICH OPTION, in the agent's own words. A number would \
             mean something different on every screen.\n{USAGE}"
        ))
    })?;

    let mut conn = connect()?;
    // ⚠⚠⚠⚠⚠ AND THE PANE IS RESOLVED THROUGH THE DOOR EVERY OTHER PANE VERB USES — register item
    // 687, which is item 542's claim arriving at the verb next door. Until here this verb handed
    // what was typed straight to `build_call`, and the published grammar declares this argument an
    // `int`: a NAME was refused as a TYPE ERROR before the daemon was ever asked, and a NUMBER went
    // out unresolved to be read against whatever window the caller happened to be standing in.
    //
    // ⚠⚠ The asymmetry is what made it worth fixing here rather than leaving to the register: the
    // surface that SHOWS a person the dialog (`sprag panes`, `sprag agent`) reaches every window of
    // the session, so the only verb that could not reach was the one that acts on what they read.
    let site = resolve_pane(&mut conn, session.as_deref(), &pane, "answer-pane")?;
    // ⚠ THE CALL IS BUILT FROM THE DAEMON'S OWN PUBLICATION, `orchestrate`'s discipline: this
    // binary holds no second list of the `answer` form's argument names.
    //
    // ⚠⚠ THE PERSON STILL TYPES TWO FLAGS, and the consent becomes a LIST OF ONE here. The wire
    // takes a list because a RUN is declared in advance and left alone, so its caller has to be
    // able to write down every decision a turn might need. This verb is typed by somebody LOOKING
    // AT the dialog it answers, quoting that screen — `--asked` and `--answer` is what they have,
    // and a list at this prompt would be a person writing rules for questions they have not seen.
    // The MCP mouth's `answer_pane` makes the same trade for the same reason.
    //
    // ⚠ The one clause is spelled as JSON because that is how the published grammar offers a list
    // (each occurrence of the flag is one element) — see `PublishedArg::element`, which checks it
    // against the very fields this daemon published rather than passing it through.
    let forms = published_forms(
        &mut conn,
        session.as_deref(),
        sprag_host::plugins::RUN_ACTION,
    )?;
    let clause = serde_json::json!({
        sprag_host::plugins::CONSENT_ASKED_KEY: asked,
        sprag_host::plugins::CONSENT_ANSWER_KEY: answer,
    })
    .to_string();
    let call = sprag_rpc::build_call(
        &forms,
        &[
            Flag::new("plugin", sprag_host::plugins::PluginName::Answer.wire_str()),
            Flag::new("pane", site.id.to_string()),
            Flag::new(sprag_host::plugins::CONSENT_KEY, clause),
        ],
    )
    .map_err(|error| bad_input(&format!("answer-pane: {error}\n{USAGE}")))?;
    // ⚠⚠ AND THE REQUEST SAYS WHICH WINDOW THE PANE WAS FOUND IN — [`site_invoke`], `orchestrate`'s
    // door and for its reason: resolving session-wide answers WHICH pane and does not carry that
    // answer to a daemon whose `require_pane_in` reads ONE window's pane pool.
    let started = invoke_action(
        &mut conn,
        site_invoke(
            session.as_deref(),
            &site,
            sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            call,
        ),
    )?;
    let id = started
        .as_u64()
        .ok_or_else(|| bad_input("answer-pane: the daemon's answer was not a run id"))?;
    let finished = wait_for_run(&mut conn, session.as_deref(), id)?;
    print!("{}", render_run(&finished));
    Ok(())
}

/// WHICH SURFACE `show-grammar` is asking, and the flag that names it.
///
/// # ⚠⚠ The plugin host was unreachable from this verb for two rounds
///
/// A verb's grammar belongs to the surface that SERVES it — that is why the answer is a per-surface
/// slot rather than one global table, and `show-grammar` narrows by surface for the same reason.
/// It knew two: the multiplexer and a pane. The plugin host has published its own `action_grammar`
/// since R353, when a derived audit found it, and **nothing taught the door about it** — so the one
/// verb whose whole job is *"ask the daemon how to call it"* could not be pointed at the loop.
///
/// The list is here rather than derived from `sprag_host::wire::SURFACES` because a surface's PATH
/// is not a function of its tag alone: a pane's is per-instance, hanging under a `pane_<id>`
/// container that has to be resolved first. What is derived is the COVERAGE —
/// `every_surface_this_crate_serves_is_reachable_from_show_grammar` requires every sprag-authored
/// entry of `SURFACES` to be named here, so a fourth one fails a test instead of quietly becoming
/// undiscoverable the way this one did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GrammarSurface {
    /// The multiplexer — sessions, windows, panes. The default, because it is where most verbs are.
    Mux,
    /// A pane's input surface — how to type into one.
    PaneInput,
    /// The plugin host — how to start, watch and stop a bounded loop.
    Plugins,
}

impl GrammarSurface {
    /// The flag that names this surface, or [`None`] for the default one — which is named by
    /// saying nothing.
    const fn flag(self) -> Option<&'static str> {
        match self {
            Self::Mux => None,
            Self::PaneInput => Some("--pane"),
            Self::Plugins => Some("--plugins"),
        }
    }

    /// The surface a flag names, or [`None`] for a word that is not one of them.
    ///
    /// Derived from [`ALL`](Self::ALL) rather than matched a second time, so an arm added to the
    /// enum is a flag this verb accepts and lists without another edit — the inverse being written
    /// twice is how a door grows a spelling nothing reaches.
    fn of_flag(flag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|surface| surface.flag() == Some(flag))
    }

    /// Every flag this verb takes, for the message that lists them.
    fn flags() -> Vec<&'static str> {
        Self::ALL.iter().filter_map(|it| it.flag()).collect()
    }

    /// The scene TAG whose grammar this asks — the half that is held against `SURFACES`.
    const fn tag(self) -> &'static str {
        match self {
            Self::Mux => sprag_host::MUX_TAG,
            Self::PaneInput => sprag_host::INPUT_TAG,
            Self::Plugins => sprag_host::PLUGINS_TAG,
        }
    }

    /// Every surface this verb can be pointed at.
    const ALL: [Self; 3] = [Self::Mux, Self::PaneInput, Self::Plugins];
}

/// The sentence to refuse with when the DAEMON publishes a run argument spelled like this CLI's own
/// `--wait`, or [`None`] while the two vocabularies are disjoint.
///
/// # ⚠ A REFUSAL, NOT A RENAME, and a free function rather than an `if` inside the verb
///
/// [`OWN_FLAGS`] are this command's own and every other flag is the daemon's, so the day the wire
/// publishes an argument of one of those names the two meanings collide silently — one would win
/// and nobody would be told. Refusing is the only answer that does not require this binary to have
/// guessed right about a daemon it did not compile with.
///
/// It is a function because the branch is otherwise reachable only from a daemon this workspace
/// cannot build: a test would have to serve a doctored grammar to drive one `if`. Narrowing the
/// ROLE to "given these forms, is there a collision?" makes the claim a unit test — the move R334
/// recorded when a rule about ORDER could not be faked through its trait.
///
/// ⚠ It walks the TABLE and not a list written here, so a flag added to this command is checked by
/// being added there — item 855 added the second one and this function did not change.
fn own_flag_collision(forms: &[PublishedForm]) -> Option<String> {
    OWN_FLAGS.iter().find_map(|own| {
        forms
            .iter()
            .flat_map(|form| form.args.iter())
            .any(|arg| sprag_rpc::call::same_name(&arg.name, own.name))
            .then(|| {
                let (flag, instead) = (own.name, own.instead);
                format!(
                    "orchestrate: this daemon publishes a run argument called {flag:?}, which is \
                     also this command's own flag for {instead}"
                )
            })
    })
}

/// What the first usage line says after the plugin and its arguments — one `[--name]` per row of
/// [`OWN_FLAGS`], in table order.
///
/// ⚠ Built rather than spelled — register item 864. See [`own_flag_lines`].
fn own_flag_summary(flags: &[OwnFlag]) -> String {
    flags
        .iter()
        .map(|own| format!(" [--{}]", own.name))
        .collect()
}

/// The usage paragraph describing this command's own flags — one line per row of [`OWN_FLAGS`].
///
/// # ⚠⚠ Built from the table, never spelled — register item 864
///
/// When item 855 added `--dry-run`, the collision check walked the table while the usage named the
/// two by hand. The table could then gain a row the usage never mentioned, and a caller reading
/// `--help` would not know the flag existed — which is item 855's own defect, a fact kept in a
/// second place, reappearing inside 855's repair. Split out as a function for the reason
/// [`own_flag_collision`] is one: it makes the claim a unit test rather than a captured stdout.
///
/// ⚠⚠ AND THE TABLE IS THE ARGUMENT, which is what makes the claim testable AT ALL. Reading
/// `OWN_FLAGS` directly, a gate can only compare the usage against today's two rows — and a printer
/// that spells those two by hand produces the very same string, so the mutation that matters
/// (spelling them) stays GREEN. Handed a table, a test can serve a THIRD row no printer knows, and
/// only a builder that walks it can answer. Measured 2026-09-03: the first shape of this gate
/// passed its own mutation.
fn own_flag_lines(flags: &[OwnFlag]) -> String {
    flags
        .iter()
        .map(|own| format!("  --{} {}\n", own.name, own.does))
        .collect()
}

/// One usage block per form, under the word that selects it — built from what the daemon answered.
///
/// ⚠ Returns the text rather than printing it, so the claims about it are unit-testable — the
/// caller does the one `print!`. `own` is [`OWN_FLAGS`] in every shipping call; it is a parameter
/// for the reason [`own_flag_lines`] gives, and that reason is a measured one.
fn orchestrate_usage(
    forms: &[PublishedForm],
    selector: &Option<String>,
    own: &[OwnFlag],
) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "sprag orchestrate PLUGIN [--ARG VALUE]…{}\n",
        own_flag_summary(own),
    );
    out.push_str(
        "\n  Start a bounded loop and print its run id. Every run is guardrail-bounded: it stops\n  \
         at its iteration ceiling or its cost ceiling, whichever binds first, and `sprag\n  \
         cancel-run` stops it sooner.\n\n",
    );
    out.push_str(&own_flag_lines(own));
    out.push('\n');
    for form in forms {
        let word = selector
            .as_ref()
            .and_then(|selector| {
                form.args
                    .iter()
                    .find(|arg| &arg.name == selector)
                    .and_then(|arg| arg.words.as_ref())
                    .and_then(|words| words.first())
            })
            .cloned()
            .unwrap_or_default();
        let _ = writeln!(out, "  {word}");
        let _ = writeln!(out, "    {}", form.usage());
    }
    out.push_str(
        "\n  The forms above are what THIS daemon publishes (`sprag show-grammar run`), not a list\n  \
         compiled into this binary. A value beginning with a dash needs --name=value.\n",
    );
    out
}

/// The usage a refusal prints under itself — the SELECTED form alone when the caller chose one,
/// because reprinting four forms buries the one they were typing.
fn usage_for(
    forms: &[PublishedForm],
    flags: &[Flag],
    selector: &Option<String>,
    words: &[String],
) -> String {
    let chosen = selector.as_ref().and_then(|selector| {
        let word = flags
            .iter()
            .find(|flag| sprag_rpc::call::same_name(&flag.name, selector))
            .and_then(|flag| flag.value.as_deref())?;
        forms.iter().find(|form| {
            form.args.iter().any(|arg| {
                &arg.name == selector
                    && arg
                        .words
                        .as_ref()
                        .is_some_and(|it| it.iter().any(|w| w == word))
            })
        })
    });
    match chosen {
        Some(form) => format!("  sprag orchestrate {}", form.usage()),
        None => format!("  sprag orchestrate <{}> …", words.join("|")),
    }
}

/// `runs [-t SESSION]`: every loop this daemon holds, and how the finished ones ended.
///
/// # Why the scope is PRE-FLIGHTED, and what it used to answer instead
///
/// These four verbs pass `-t` through as a request's out-of-band scope. A session name nobody has
/// is refused by the daemon as *nothing is served at that path*, because that is the only thing an
/// unresolvable scope can look like from the wire — and this client reads that fault as VERSION
/// SKEW, which for every other cause it is. So a typo used to come back as one of two wrong
/// answers, measured 2026-08-17 against a daemon built from HEAD that serves all four paths:
///
/// ```text
///                       -t work (a session that exists)   -t nosuch
/// orchestrate           names the plugins                 host rpc error: NoExternalAtPath
/// runs                  no runs (start one ...)           host rpc error: NoExternalAtPath
/// cancel-run 999        no run 999 is in flight           "... is older than this build of
/// stand-down 999        no run 999 is in flight            sprag. Restart it: `sprag kill-server`"
/// ```
///
/// ⚠⚠⚠⚠ **The second pair is worse than the leaked variant name.** A leaked variant is ugly and
/// admits it failed; that sentence is confident and wrong — it diagnoses skew that is not there and
/// tells the operator to end EVERY session on the machine, in answer to a mistyped word. On a host
/// running the debt loop, following it kills live runs.
///
/// [`connect_scoped`] is the pre-flight every window and pane verb already made, and the one item
/// 425 gave `processes` / `resources` for this same half of the same defect. Found by the sweep
/// that item asked of every other verb publishing `-t`: nine readers probed, eight already clean.
/// **HOW ONE CONVERSATION IS ATTACHED TO ONE RUN** — register item 865's ⑷.
///
/// # ⛔⛔⛔⛔⛔ The two ends are different acts, so they are two words and not one
///
/// A conversation that ASKED for a run may cancel it, answer for it, and be asked before a
/// promotion kills it — item 865's whole subject. A conversation the run is DRIVING is the thing
/// being typed into; it decides nothing about the run and a promotion killing that run kills its
/// work. Merging them would put *go and ask them* and *this is being done to you* under one word.
///
/// ⚠ [`Both`](Self::Both) is kept rather than folded into `Asked` for the reason
/// [`Blind`](sprag_terminal::doctor::Blind)'s bar admits: a conversation that asked for a run onto
/// its OWN pane is on both ends at once, and a reader told only *you asked for it* would not know
/// that stopping it stops what is typing into them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stake {
    /// This conversation asked for the run.
    Asked,
    /// The run is driving the pane this conversation is living in.
    Driven,
    /// It asked for the run AND the run is driving its pane.
    Both,
}

impl Stake {
    /// Every stake, so the walk that holds the three sentences apart needs no list somebody has to
    /// remember to extend.
    ///
    /// ⚠ `#[cfg(test)]` because the product never walks these — it classifies one run at a time —
    /// and a constant carried into the binary for a gate's sake is dead weight clippy is right
    /// about. It is a LIST rather than a literal in the gate for the usual reason: a fourth arm
    /// joins the walk in the compile that adds it.
    #[cfg(test)]
    const ALL: [Self; 3] = [Self::Asked, Self::Driven, Self::Both];

    /// What being on this end MEANS to whoever is reading — a sentence about what they may do,
    /// because *which end* is only useful as a difference in what happens next.
    const fn describe(self) -> &'static str {
        match self {
            Self::Asked => {
                "YOU ASKED FOR IT — this run is yours to cancel, to answer, and to be asked about \
                 before anybody ends it"
            }
            Self::Driven => {
                "IT IS DRIVING YOU — this run is typing into your pane. You did not ask for it and \
                 stopping it is not yours to decide, but its work is what is happening to you"
            }
            Self::Both => {
                "BOTH ENDS — you asked for this run and it is driving your pane, so stopping it \
                 stops what is typing into you"
            }
        }
    }

    /// The stake conversation `me` in pane `seat` has in `run`, or [`None`] for a run it is on
    /// neither end of.
    ///
    /// # ⚠⚠⚠ `me` is an [`Option`] because a SHELL pane can be driven
    ///
    /// The asking end is a conversation and a shell has none, so that half is unanswerable there —
    /// but a run drives a pane, and an `orchestrator` run driving a shell is the ordinary case. A
    /// version of this that refused a seatless caller outright would answer *nothing* about a pane
    /// something is demonstrably typing into.
    ///
    /// # ⚠⚠ The driven end is RUNNING runs only, and that is the daemon's own rule
    ///
    /// `Progress::driving` still names the pane an ENDED run drove — item 540's point, asked of
    /// history — and the daemon's `driven` marker on the panes listing filters on the state for
    /// exactly this reason: *"reading it here would say somebody is driving a pane nobody is"*.
    /// The asking end carries no such filter, on the same source's terms: *whose work is this* is a
    /// question an ended run answers perfectly well.
    fn of(run: &Value, me: Option<&str>, seat: u64) -> Option<Self> {
        let asked =
            me.is_some_and(|me| run[sprag_host::plugins::RUN_ASKED_BY_KEY].as_str() == Some(me));
        let driven = run["state"]["status"].as_str()
            == Some(sprag_host::plugins::RunStatus::Running.wire_str())
            && run[sprag_host::plugins::RUN_DRIVING_KEY].as_u64() == Some(seat);
        match (asked, driven) {
            (true, true) => Some(Self::Both),
            (true, false) => Some(Self::Asked),
            (false, true) => Some(Self::Driven),
            (false, false) => None,
        }
    }
}

/// ⛔⛔⛔⛔⛔ `my-runs [-t SESSION]`: **WHICH RUNS THIS CONVERSATION IS ON** — register item 865's
/// ⑷, and the one direction every other half of that item left unbuilt.
///
/// # ⚠⚠⚠⚠⚠ What was measured, and why asking somebody else is not an answer
///
/// Item 865 was opened because a promotion about to kill a live run had to find its owner by
/// MESSAGING THREE SESSIONS — five messages, forty minutes. Its ⑴⑵⑶ gave the RUN a mouth for its
/// asker and ⑸ gave the PANE a mouth for its occupant. Every one of those is answered by somebody
/// looking at a run or a pane **from outside**. The half that stayed open is the one the
/// `watching-zenoh` watcher reported in its own words when the same promotion reached it:
///
/// > *"제가 어느 sprag run 에 매달려 있는지 «제 쪽에서 볼 수 있는 계기가 없습니다» … 그래서
/// > 「제 것이 아니다」라고도 말하지 않겠습니다."*
///
/// That refusal was CORRECT — the rule a peer wrote for this item is *"a session that does not own
/// a run approving it is not approval, it is a guess"* — and it is what this verb exists to make
/// unnecessary. A conversation that can ask this can say *mine* or *not mine* without anybody
/// answering for it.
///
/// # ⛔⛔ IT TAKES NO SUBJECT, and that is a safety property rather than a missing feature
///
/// Item 871 measured the shape: **a caller may POINT and may not NAME.** `$SPRAG_PANE` is the
/// daemon's own stamp on this process, so a caller reading it repeats an identity it was given;
/// a `my-runs <SESSION>` argument would let anybody assert somebody else's. The subject here is
/// always the caller, and the conversation is resolved BY THE DAEMON from the pane.
///
/// # ⚠⚠ It keys on the CONVERSATION and never on the pane, for the asking end
///
/// `PersistedRun::opened_by_session`'s own doc: *"pane 3 comes back as pane 3, but the successor
/// has no way to know whether the thing sitting in it is the asker or a stranger who booted into
/// the same seat"*. Runs also carry `opened_by` (a pane) and filtering on it would have needed no
/// second read at all — and would answer wrongly for exactly the case item 865 was opened on.
///
/// # ⚠ TWO SLOT READS, and what the second one cannot tear
///
/// The first read answers *who is asking* and the second *what that conversation is on*. They are
/// not the torn read [`layout`] is a separate verb to avoid: what is carried between them is the
/// caller's own identity, which does not change under the caller — and if the pane were respawned
/// mid-command the answer would be about a conversation that has ended, which the run rows below
/// say for themselves.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] when `$SPRAG_PANE` is unset — the caller is a shell that is not
/// a pane of this daemon, so there is nobody to answer about — and whatever [`resolve_pane`]
/// refuses a stale one with.
fn my_runs(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "my-runs")?;
    if let Some(extra) = rest.first() {
        return Err(bad_input(&format!(
            "my-runs: unexpected argument {extra:?} (it takes only -t SESSION). It answers about \
             the CALLER and takes no subject: a caller may point at itself and may not name \
             somebody else (register item 871)."
        )));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    // ⛔ WHO IS ASKING, OR A REFUSAL — never an empty list. A shell outside this daemon has no
    // conversation, and answering it *you are on no run* would be a claim about a caller nothing
    // here can identify: exactly the *"「제 것이 아니다」라고도 말하지 않겠습니다"* case, answered
    // wrongly.
    let raw = std::env::var(sprag_host::PANE_ENV_VAR).map_err(|_| {
        bad_input(&format!(
            "my-runs: ${} is not set, so this command cannot tell which conversation is asking. \
             It answers about the caller, and a shell that is no pane of this daemon has no \
             conversation to answer about — run it inside the pane whose runs you are asking about.",
            sprag_host::PANE_ENV_VAR,
        ))
    })?;
    let site = resolve_pane(&mut conn, session.as_deref(), &raw, "my-runs")?;
    // ⚠ THE SAME SCOPE `panes` READS, for `panes`' stated reason (item 759): a listing narrowed at
    // one end and addressed at the other is neither window's.
    let listed: Value = query_slot(
        &mut conn,
        match site.window.as_deref() {
            Some(window) => windowed_params(
                session.as_deref(),
                mux_action_path(PANES_SLOT),
                Some(window),
            ),
            None => here_params(session.as_deref(), mux_action_path(PANES_SLOT)),
        },
    )?;
    let me = listed
        .as_array()
        .into_iter()
        .flatten()
        .find(|pane| pane["id"].as_u64() == Some(site.id))
        .and_then(|pane| pane[sprag_host::wire::PANE_SESSION_KEY].as_str());
    let answer = query_slot(
        &mut conn,
        scoped_params(
            session.as_deref(),
            sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
        ),
    )?;
    let entries = answer
        .as_array()
        .ok_or_else(|| bad_input("my-runs: the daemon's answer was not a list of runs"))?;
    // ⛔⛔⛔ A PANE WITH NO CONVERSATION SAYS SO, and says WHICH HALF that costs. Silence here would
    // read as *you asked for nothing*, which is a claim, where the truth is that the asking end
    // cannot be asked at all from a shell.
    match me {
        Some(id) => println!("my-runs: pane {} holds conversation {id}", site.id),
        None => println!(
            "my-runs: pane {} holds NO conversation (it is not an agent's pane), so WHAT THIS \
             CONVERSATION ASKED FOR cannot be answered here — only what is driving this pane",
            site.id,
        ),
    }
    let mine: Vec<(&Value, Stake)> = entries
        .iter()
        .filter_map(|run| Stake::of(run, me, site.id).map(|stake| (run, stake)))
        .collect();
    if mine.is_empty() {
        println!(
            "  NO RUN of this daemon has this caller on either end — nothing asked for by this \
             conversation, and nothing driving this pane. That is an ANSWER: it is what lets a \
             caller say `not mine` instead of `I cannot tell`"
        );
        return Ok(());
    }
    // ⛔⛔⛔ THE HEADING AND NOT THE BLOCK, and it is `render_run`'s OWN heading rather than a
    // second opinion on how a run is named. Measured against the live daemon while this verb was
    // written: printing the block gave **one run and ninety lines**, forty-eight of them journal
    // steps — a caller asking *am I on this* had to read a run's whole history to find out that it
    // was. `sprag runs` is the verb for the rest, and it is one command away.
    for (run, stake) in mine {
        println!("  {}", stake.describe());
        let block = render_run(run);
        println!("  {}", block.lines().next().unwrap_or_default().trim());
    }
    println!("  (`sprag runs` has each of these in full)");
    Ok(())
}

fn runs(args: Vec<String>) -> io::Result<()> {
    let (session, args) = scope_and_rest(args, "runs")?;
    if let Some(extra) = args.first() {
        return Err(bad_input(&format!(
            "runs: unexpected argument {extra:?} (it takes only -t SESSION)"
        )));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let answer = query_slot(
        &mut conn,
        scoped_params(
            session.as_deref(),
            sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
        ),
    )?;
    let entries = answer
        .as_array()
        .ok_or_else(|| bad_input("runs: the daemon's answer was not a list of runs"))?;
    if entries.is_empty() {
        println!("no runs (start one with `sprag orchestrate`)");
        return Ok(());
    }
    for run in entries {
        print!("{}", render_run(run));
    }
    // ⛔⛔⛔⛔⛔ AND THE SENTENCE TABLE OVER THE RUNS JUST LISTED — register item 889(1). Each
    // run's own line says which of ITS prompts stuck; item 889's subject is the comparison ACROSS
    // runs (*the turn prompt sticks fifteen times as often as the brief*), and until this the only
    // way to that number was a heredoc over the loop's log files — the files items 887 and 888
    // measured as unreliable. The done-when asks for it to come out of ROWS, and these are them.
    //
    // ⚠⚠ It says the population as well as the rates, and that is the load-bearing half: measured
    // 2026-09-04, 206 of this loop's 212 rows carry an all-zero table written by a daemon that had
    // no counter, and adding those would put a denominator of 212 under data from three.
    for line in sprag_host::plugins::SaidAcrossRuns::of_runs(entries).lines() {
        println!("{line}");
    }
    Ok(())
}

/// THE STEPS A RUN TOOK, one per line — the account a person reads to find where a loop went wrong.
///
/// A column layout rather than the agent's prose, on the split every other pair of renderers here
/// keeps: a person scans a hundred rows for the odd one out, and an agent reads sentences.
fn render_journal(run: &Value) -> String {
    let Some(steps) = run[sprag_host::plugins::RUN_JOURNAL_KEY].as_array() else {
        return String::new();
    };
    steps
        .iter()
        .map(|step| {
            format!(
                "    {:>4}  {:>8} {:<7} {:<9} {}\n",
                step["iteration"].as_u64().unwrap_or_default(),
                step["cost"].as_u64().unwrap_or_default(),
                step["unit"].as_str().unwrap_or("steps"),
                step["verdict"].as_str().unwrap_or("?"),
                step["note"].as_str().unwrap_or(""),
            )
        })
        .collect()
}

/// ⛔⛔⛔⛔⛔ **WHETHER A RUN A BOOT RESCUED EVER WENT BACK TO WORK** — register item 774, as a
/// status-line clause; empty for a run nobody put back and for one that is plainly working.
///
/// # ⚠⚠⚠⚠⚠ The absence of a delivery count IS the reading, and that is not a shortcut
///
/// `RUN_DELIVERED_KEY` and its two neighbours are published **only when at least one of them is
/// non-zero** — `plugins::run_to_json` says why: a plugin that composes nothing (`pipe`,
/// `orchestrator` relay words they did not write) would otherwise publish three zeroes that read
/// as *it typed nothing*. So a run that has delivered nothing carries **no delivery key at all**,
/// and this is the mechanical reason item 774's row was silent: there was no number to be silent
/// ABOUT. The key's absence is therefore exactly *nothing has been typed*, read structurally.
///
/// ⚠⚠ **AND THE ONE THING THAT ABSENCE CANNOT TELL APART IS SAID OUT LOUD**, rather than left for
/// a reader to complete: a plugin that composes no prompts looks identical here. The clause names
/// both readings, which is this workspace's rule that an unclassified case is stated and never
/// glossed.
///
/// ⚠ THREE ANSWERS AND NOT TWO. A rescued run that has not taken a step yet has typed nothing for
/// a reason nobody should act on — the boot has just handed it over — and folding it in with a run
/// that has stepped and stayed silent would send somebody to a pane at the one moment there is
/// nothing to see.
///
/// # ⛔⛔⛔⛔⛔ FOUR, and the one added is the window this clause could not see — register item 815
///
/// The reading above rests on the delivery count being THIS driver's. For the first stretch of a
/// rescued run's life it is not: `RunRegistry::restore` fills the row's counters out of the
/// predecessor's log on purpose (items 606 and 616), and nothing overwrites them until a driver of
/// this daemon takes a step. **Measured 2026-09-01**: a rescued run whose pane came back a plain
/// shell published `running — 2 iterations` and `1 prompt(s) delivered` while its new driver had
/// taken no step and typed nothing — so the stale count silenced this clause on exactly the run
/// item 774 was filed over.
///
/// So `RUN_INHERITED_KEY` is read FIRST and short-circuits the rest: when the numbers are a dead
/// daemon's, neither the delivery count nor the step count is evidence about anything, and the only
/// honest sentence is the one that says so.
///
/// ⚠⚠ **IT SPEAKS FOR THE WHOLE ROW, WHICH IS WHY THE DELIVERY LINE BELOW IS LEFT ALONE.** That
/// line is what the run typed BEFORE the boot and item 606 restored it deliberately; qualifying it
/// as well would put the same fact in two mouths. The residue, stated rather than hidden: a reader
/// who skips the status line still meets *N prompt(s) delivered, all of them on that pane* with
/// nothing beside it — which is why this clause names the counts rather than only the silence.
fn resumed_clause(run: &Value, state: &Value) -> String {
    if run[sprag_host::plugins::RUN_RESUMED_KEY].as_bool() != Some(true) {
        return String::new();
    }
    // ⛔⛔⛔⛔⛔ FIRST, AND THE ORDER IS THE REPAIR — register item 815. Every test below reads a
    // counter, and none of these counters is this driver's until it has spoken.
    if run[sprag_host::plugins::RUN_INHERITED_KEY].as_bool() == Some(true) {
        return " · ⚠ this run was put back by a boot and nothing has driven it since — every \
                count on this row is the predecessor's, so neither the steps nor the deliveries \
                say whether it has typed anything at its pane"
            .to_owned();
    }
    if run[sprag_host::plugins::RUN_DELIVERED_KEY]
        .as_u64()
        .is_some()
    {
        return String::new();
    }
    match state["iterations"].as_u64().unwrap_or_default() {
        0 => " · this run was put back by a boot and has not taken a step yet".to_owned(),
        steps => format!(
            " · ⚠ this run was put back by a boot and has taken {steps} step(s) with no delivery on \
             this row — either it has typed nothing at its pane since, or its plugin composes no \
             prompts at all"
        ),
    }
}

/// HOW MANY OF ITS PEER'S QUESTIONS a run answered on the caller's consent, as a clause — empty
/// when it answered none.
///
/// ⚠⚠ Read from the same key whether the run is RUNNING or DONE, because a person watching a loop
/// approve things on their behalf needs it most while there is still time to cancel. The key is
/// always present and `0` is the common answer, so the clause and not the key is what disappears.
fn render_answered(state: &Value) -> String {
    match state[sprag_host::plugins::RUN_ANSWERED_KEY]
        .as_u64()
        .unwrap_or_default()
    {
        0 => String::new(),
        1 => "\n  it answered 1 question for you — see the journal for which".to_owned(),
        many => format!("\n  it answered {many} questions for you — see the journal for which"),
    }
}

/// 🎯🎯🎯🎯🎯 HOW MANY NEXT CHECKPOINTS A RUN COUNTED RATHER THAN TOOK, as a clause — empty for a
/// run that set none aside and for a plugin that has no such choice to make.
///
/// # ⚠⚠⚠⚠⚠ Why the cap that produced this number needed a mouth at all
///
/// Register item 833(2), the owner's decision of 2026-09-02. Bounding how far a run may re-aim
/// itself is what stops a loop paying its own debt for ever — measured here that same day: eleven
/// register items closed in twenty-two commits, **nine of them registered the same day**, and the
/// forty-one standing that morning lost exactly one.
///
/// The DANGER the item names in its own words is that the cap becomes a quiet way of losing
/// findings: *"a cap without this number is indistinguishable from a loop that never found
/// anything"*. A count that reached the wire and died here would be exactly that — **a fact that
/// reaches the wire and dies at the mouth somebody actually reads**, which is the shape this file
/// has now written down four times.
///
/// ⚠⚠ READ FROM THE SAME KEY WHETHER THE RUN IS RUNNING OR DONE, on `render_answered`'s argument
/// and with its own twist: a person who sees this climbing on a LIVE run is the one who can still
/// widen the brief. Once the run is over the number is only a post-mortem.
///
/// ⚠ THE CLAUSE DISAPPEARS ON ZERO, not the key. `0` is a real claim — *it had the budget and never
/// had to spend it* — and it is what a healthy run says; printing a line about it on every row
/// would bury the rows where it is not zero.
/// 🎯🎯🎯🎯🎯 **HOW MANY TIMES THIS RUN CHANGED DIRECTION WITH NOBODY CHECKING** — the owner's
/// decision of 2026-09-03, register item 847, and [`render_deferred`]'s twin.
///
/// # ⛔⛔⛔⛔⛔ The clause that is printed on a NUMBER, not on its absence
///
/// The loop's document names the program that decides whether a proposed next checkpoint may be
/// taken, and the template ships that slot empty on purpose — a repository gets the machinery
/// before it has a judgement to put in it. It was inert **quietly**, which is what this ends: a run
/// with the bound switched off and a run with it satisfied printed the same row.
///
/// ⚠⚠ **AND THE CLAUSE DISAPPEARS ON ZERO, EXACTLY AS `render_deferred`'s DOES.** `0` is the
/// healthy claim — *every direction this run took was checked* — and a line about it on every row
/// would bury the rows where it is not zero. The absence of the KEY is a third thing again (*this
/// plugin does not re-aim itself*) and prints nothing either.
///
/// ⚠ It says what to DO, because a number a reader cannot act on is a number they scroll past: the
/// remedy is naming a checker in the kind's own document, and nothing else here can say that.
fn render_unchecked(state: &Value) -> String {
    match state[sprag_host::plugins::RUN_UNCHECKED_KEY].as_u64() {
        None | Some(0) => String::new(),
        Some(1) => "\n  it changed direction ONCE with nobody checking — this run's kind names no \
                    `successor_check`, so the bound on where a reflection may aim it is switched \
                    off; name one in that kind's own document to turn it on"
            .to_owned(),
        Some(many) => format!(
            "\n  it changed direction {many} times with nobody checking — this run's kind names no \
             `successor_check`, so the bound on where a reflection may aim it is switched off; \
             name one in that kind's own document to turn it on"
        ),
    }
}

/// **HOW MANY NEXT CHECKPOINTS A RUN COUNTED RATHER THAN TOOK, AND WHY** — register items 833 and
/// 839.
///
/// # ⛔⛔⛔⛔⛔ The clause named a reason the number does not carry
///
/// Two different things set a proposal aside — the run spent the re-aiming budget its document
/// gives it, and the kind's `successor_check` refused what its agent named — and the document keeps
/// ONE total on purpose, because two would make every reader add them up. It puts the reason in the
/// ending's own word (`capped` against `unadmitted`), which is right for a run that has ENDED.
///
/// **Measured 2026-09-03 against the live loop daemon**: run 189 was still going, had set THREE
/// aside, every one of them a refusal and not one at the cap — and this clause said *"at its depth
/// cap"*. There is no ending word on a running row, so the sentence had supplied the missing half
/// itself, and it supplied the wrong one for all three.
///
/// ⚠⚠ **THE REMEDIES ARE OPPOSITE, which is why this is worth a second number rather than a longer
/// sentence.** A proposal set aside at the cap is registered and the next run may take it; one the
/// classifier refused will be refused again, because it is not in the set that kind admits.
///
/// ⚠ The qualifying clause is present only when the daemon SAID how many were refusals: an older
/// daemon omits the key, and this then says the total without claiming a reason — which is the
/// honest reading and the one this function had no way to give before.
fn render_deferred(state: &Value) -> String {
    let Some(many) = state[sprag_host::plugins::RUN_DEFERRED_KEY]
        .as_u64()
        .filter(|deferred| *deferred > 0)
    else {
        return String::new();
    };
    let (one, them, counted) = if many == 1 {
        (
            "1 next checkpoint".to_owned(),
            "it",
            "its agent proposed one and this run counted it instead of taking it".to_owned(),
        )
    } else {
        (
            format!("{many} next checkpoints"),
            "them",
            "its agent proposed them and this run counted them instead of taking them".to_owned(),
        )
    };
    // ⛔⛔⛔ WHY, WHEN THE DAEMON SAID — and the two answers are two different things to do next.
    let why = match state[sprag_host::plugins::RUN_UNADMITTED_KEY].as_u64() {
        None => String::new(),
        Some(0) => format!(
            "\n  all of that is the re-aiming budget this run's own document gives it — nothing was \
             refused, so what it set aside is registered and a later run may take {them}"
        ),
        Some(refused) if refused >= many => {
            "\n  every one of those was REFUSED by this run's kind \
             — naming the same thing again gets the same answer, because it is not in the set that \
             kind admits"
                .to_owned()
        }
        Some(refused) => format!(
            "\n  {refused} of those were REFUSED by this run's kind and the rest ran out of the \
             re-aiming budget, and the two want opposite things next: a refused one is not in the \
             set that kind admits, and a budgeted one is registered for a later run"
        ),
    };
    format!(
        "\n  it set {one} aside — {counted}, so look for {them} wherever this run's kind registers \
         such things{why}"
    )
}

/// WHAT THE PEER IS ASKING and why the run did not answer it, for a run that ended `blocked` —
/// empty for every other run.
///
/// # ⚠⚠⚠ Why this is the mouth that most had to grow it
///
/// A `blocked` run exists to be ANSWERED, and the person reading this line is the one who has to
/// answer it. Printing the word alone told them a dialog is up and left them to go find the pane,
/// read the menu, and work out which option a bare Enter would take — every part of which this
/// daemon had already parsed and published. That is the failure the `stopped` clause above names
/// in its own comment: **a fact that reaches the wire and dies at the mouth somebody reads.**
///
/// ⚠ The REASON is rendered from [`sprag_plugin::Refusal`]'s own sentence rather than printed as
/// its wire word. The word is for a client that branches; a person needs the remedy, and the type
/// is where that sentence lives so both mouths say the same one.
fn render_asking(outcome: &Value) -> String {
    let asking = &outcome[sprag_host::plugins::RUN_ASKING_KEY];
    let Some(why) = asking[sprag_host::plugins::RUN_WHY_KEY].as_str() else {
        return String::new();
    };
    // ⚠ Through the host's projection, so this mouth and the agent-facing one say the SAME
    // sentence for the same word — and a word this build does not know still prints, as itself.
    format!(
        "\n  {}{}",
        sprag_host::plugins::refusal_sentence(why),
        render_question(asking, "    "),
    )
}

/// THE QUESTION A PEER IS ASKING, as a person reads it — its own lines, then every option with the
/// one a bare Enter would take marked.
///
/// # ⚠⚠ One renderer, because two surfaces publish the SAME question
///
/// A run's `blocked` outcome carries it and — since R367 — so does a PANE's `agent` object, off the
/// same parse in the same instant. They were rendered in one place and read in one, so
/// `sprag agent 3` printed `blocked`, named the rule, and threw the menu away: a person was told
/// their agent was waiting and had to go look at the pane to find out what for. R369 gave the shell
/// a verb to ANSWER with, which made that silence worse — you cannot quote an option you were never
/// shown.
///
/// `indent` is the caller's: a run nests its question under an outcome line and a pane under its
/// own row, and a block that chose its own would be misaligned on one of them.
fn render_question(asking: &Value, indent: &str) -> String {
    let mut said = String::new();
    for line in asking[sprag_host::plugins::RUN_ASKED_KEY]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        said.push_str(&format!("\n{indent}{}", line.as_str().unwrap_or_default()));
    }
    for choice in asking[sprag_host::plugins::RUN_CHOICES_KEY]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        said.push_str(&format!(
            "\n{indent}{} {}. {}",
            // ⚠ WHICH ONE A BARE ENTER TAKES, marked rather than left to be inferred: on a
            // tool-permission dialog that is the difference between confirming a command and
            // declining it, and it is the one fact a person cannot read off the option text.
            if choice["selected"].as_bool().unwrap_or_default() {
                "->"
            } else {
                "  "
            },
            choice["number"].as_u64().unwrap_or_default(),
            choice["label"].as_str().unwrap_or_default(),
        ));
    }
    said
}

/// WHICH BUILD DROVE a run, as a clause on its heading — EMPTY when it is the build this client
/// is, which is the common case and the quiet one.
///
/// # ⚠⚠⚠⚠⚠ Why silence is allowed to mean "the same build" HERE and nowhere else
///
/// Every other reader of this fact is forbidden to fill in an absence: an absent
/// [`sprag_host::plugins::RUN_BUILD_KEY`] means *nothing recorded which build this was*, and a
/// reader that took it for its own would date a dead daemon's work to its successor. That rule is
/// not broken here, because this function does not print the ABSENCE — it prints a COMPARISON it
/// resolved, against a value it knows for certain (its own [`sprag_host::wire::BUILD`]). An empty
/// clause therefore asserts *"same as this client"*, which is a positive answer, and the two cases
/// that cannot be resolved get words of their own.
///
/// # Why the clause goes on the HEADING and not under it
///
/// The line below a run's heading is its STATUS, and the line after that begins its walk. Both are
/// parsed — this repository's own outer-loop watcher takes the status by `getline` off the heading
/// and reads the walk's last line by position. A new line anywhere under the heading moves one of
/// them. What a build belongs to is the run, which is what the heading names.
fn render_build(run: &Value) -> String {
    match run[sprag_host::plugins::RUN_BUILD_KEY].as_str() {
        Some(build) if build == sprag_host::wire::BUILD => String::new(),
        // ⚠ THE ONE A READER MUST ACT ON: this run was driven by other code than the client asking
        // about it is built from, so its walk is evidence about that build and not about the tree.
        Some(build) => format!("  (driven by build {build})"),
        // ⚠ AND THE ONE THAT MUST NOT BE SILENT: a run restored from a log written before daemons
        // recorded this. It is not "the same build"; it is nobody knowing.
        None => "  (build not recorded)".to_owned(),
    }
}

/// ⛔⛔⛔⛔⛔ **WHICH RUN THIS IS**, beside the number that cannot say — register item 887.
///
/// # ⛔⛔⛔⛔⛔ It is printed on the HEAD line because the head is what a watcher copies
///
/// The number in `run {id}` is what every reader of this product has been keying its records on —
/// the loop's own watcher names its log file `run<N>.log`, and every table this repository has
/// built about itself joins that file to a row by `N`. Measured 2026-09-04: three of this daemon's
/// numbers name two runs each. So the thing that CAN identify the run has to arrive in the same
/// glance as the thing that cannot, or a watcher goes on recording the number alone.
///
/// ⚠⚠ **AND ITS ABSENCE IS SAID OUT LOUD**, `render_build`'s call one field over: a run out of a
/// log written before the stamp existed is not *the same run as whatever else bears this number*,
/// it is nobody having recorded which run it was — and a silent omission here would read as the
/// former to anyone comparing two rows.
///
/// ⚠⚠⚠ **THE EXISTING READER WAS CHECKED RATHER THAN ASSUMED**, which is the constraint
/// `render_build`'s neighbours state: the repayment skill's `watch.sh` anchors on `^run <N> ` as a
/// PREFIX and takes the run's status from the line after the heading, so a clause appended to the
/// end of the heading moves neither. Driven against a real snapshot before this shipped. It is the
/// only bracketed field on that line, which is what lets a shell reader take it with `[^][]*$`.
fn render_which_run(run: &Value) -> String {
    match run[sprag_host::plugins::RUN_WHICH_RUN_KEY].as_str() {
        Some(which) => format!("  [{which}]"),
        None => "  [which run not recorded]".to_owned(),
    }
}

/// ⛔⛔⛔⛔⛔ **WHICH REPOSITORY THIS RUN WAS FOR** — register item 890, and the third thing the head
/// line has to say now that one daemon drives several.
///
/// # ⛔⛔⛔⛔⛔ 206 of 209 rows could not answer it
///
/// [`render_build`] says which CODE drove a run and [`render_which_run`] says which RUN it was.
/// Neither says WHERE, and nothing did: measured 2026-09-04 on this daemon's own store, the
/// repository appeared only inside the `request` map, which
/// `sprag_host::runs::PersistedRun::request` drops for a finished run — correctly, and for a
/// stated reason. **So every run anybody reads named none of the three trees this daemon drives.**
/// A watcher attributing runs 194–198 could not, because the only surviving evidence was the live
/// drivers' command lines and those drivers had exited.
///
/// ⚠⚠ **ABSENT IS SAID OUT LOUD**, both neighbours' rule and here the one that matters most: a
/// silent omission would be read as *this run is mine*, by whoever is standing in a repository at
/// the time. Nobody recording it is a different fact and gets different words.
///
/// ⚠ THE HEAD LINE, beside the other two, for [`render_which_run`]'s measured reason: the thing a
/// watcher copies is the head, so an identity that arrives anywhere else is an identity that
/// arrives after the record has been written.
///
/// ⛔⛔⛔⛔⛔ **AND BEFORE THE STAMP RATHER THAN AFTER IT** — see the call in [`render_run`], which
/// carries the measurement. The repayment skill's `watch.sh` reads item 887's stamp with a pattern
/// anchored at the end of the line, so a clause appended past it blinds that reader without
/// changing a single character it can see.
fn render_tree(run: &Value) -> String {
    match run[sprag_host::plugins::RUN_TREE_KEY].as_str() {
        Some(tree) => format!("  in {tree}"),
        None => "  (tree not recorded)".to_owned(),
    }
}

/// ⛔⛔⛔⛔⛔ **WHY A RUN ENDED THE WAY IT DID**, for the two endings that are a bare word without it
/// — register item 685, and register item 594's twin one word over.
///
/// # ⚠⚠⚠⚠⚠ The word that reads as an accusation
///
/// `panicked` is the right word and is not what needs fixing: a driver that died reporting nothing
/// cannot honestly be read as any other ending (`driver_ending`'s own doc — *"SILENCE IS AN OUTCOME
/// AND IT IS NOT `converged`"*). What was wrong is that the word arrived ALONE. A watcher in another
/// repository read it as *my run hit a bug* when a `kill-server` had killed the driver — and the
/// difference was already on the wire, in the sentence `RunState::Panicked` carries, which names
/// the exit status and therefore the SIGNAL.
///
/// # ⚠⚠ Read off the state rather than composed here
///
/// The daemon wrote that sentence and it is the one authority on it; a second phrasing at this
/// mouth would be two authors of one fact — the drift `outcome_to_json` is `pub` to prevent. This
/// only decides WHERE it goes.
///
/// ⚠ Empty for `interrupted`, which carries no such key and needs none: what a person has to learn
/// about that ending is whether anybody will pick the run back up, and `withheld` beside this says
/// so (register item 737).
fn render_why_it_ended(state: &Value) -> String {
    match state[sprag_host::plugins::RUN_ERROR_KEY].as_str() {
        Some(why) if !why.trim().is_empty() => format!("\n  {}", why.trim()),
        _ => String::new(),
    }
}

/// ⛔⛔⛔⛔⛔ **WHO ASKED FOR THIS RUN** — register item 865, and this clause is never empty.
///
/// # ⚠⚠⚠⚠⚠ Why silence was the defect, and not merely a missing name
///
/// A promotion kills every run the daemon is driving, so before one somebody has to find each run's
/// owner and ask. Item 865 records what that cost when this row said nothing: **three sessions
/// messaged, five messages, about forty minutes** — and the owner had been alive and reachable the
/// whole time. The row's answer was `pane=786`, which is *where the run types*, not *who asked for
/// it*; the two differ the moment a loop replaces its agent's session, and a pane that has closed
/// answers nothing at all.
///
/// # ⚠⚠⚠⚠ The three states a reader must be able to tell apart
///
/// Before this, a run whose asker was recorded and a run whose asker was never recorded produced
/// **byte-identical rows** — both silent — so a person could not tell *nobody asked* from *nobody
/// wrote it down*, and had no way to learn which. Item 865's ⑴ says so in as many words: *「모른다」
/// also is an answer; right now the cell is missing entirely, so we cannot even tell whether asking
/// would help.* So:
///
/// * a conversation was recorded → name it, and the seat too when one is still holding it;
/// * only a seat → say the pane AND say the conversation was not recorded, because that pane is a
///   guess about an owner and a reader must know it is one;
/// * neither → say so out loud.
///
/// ⚠⚠ **THE THIRD ARM IS THE ONE THAT IS TRUE TODAY.** Measured on the live loop daemon while this
/// was written: **190 of 190 runs** carried no conversation, because the CLI door that launches them
/// sends no opener at all (the MCP surface stamps one; `sprag orchestrate` has no way to). Printing
/// nothing made a product-wide gap look like a per-run coincidence. ⚠ Do not carry that count
/// forward — re-derive the predicate, which is what does not age:
/// `jq '[.runs[] | select(.opened_by_session != null)] | length' <state>/*.runs.json`.
///
/// ⚠ It stays on the HEADING line, where the old pane clause already was: `render_run`'s own comment
/// records that this repository's outer-loop watcher reads the STATUS as the line after the heading
/// and the walk as the block's last line, so this must not become a detail line.
fn render_who_asked(run: &Value) -> String {
    let seat = run[sprag_host::plugins::RUN_OPENED_BY_KEY].as_u64();
    match run[sprag_host::plugins::RUN_ASKED_BY_KEY].as_str() {
        // The answer this row exists to give: a name somebody can be reached at.
        Some(session) => match seat {
            Some(pane) => format!("  (asked for by {session}, sitting in pane {pane})"),
            // ⚠ THE CASE ITEM 865 WAS OPENED FOR. The conversation is recorded and no pane in this
            // workspace is holding it — the asker's pane closed, or their session moved. The daemon
            // resolves the seat on every read (`PluginsExternal::seat_of`), so this is not stale
            // data; it is the honest answer, and it is still enough to go and ask.
            None => format!("  (asked for by {session}, whose pane is no longer here)"),
        },
        // ⚠⚠ A SEAT WITHOUT A NAME, AND THE ROW SAYS BOTH HALVES. The pane is real but it is a
        // guess about an owner: a seat is re-taken, and item 865's own measurement is that cwd and
        // session age do not tell one occupant from another.
        None => match seat {
            Some(pane) => {
                format!(
                    "  (asked for by pane {pane}; no conversation recorded, so that pane is a guess)"
                )
            }
            // ⚠ *asked for* verbatim, like the three arms above it: the clause a reader scans for
            // is the same clause whatever it goes on to say, and its own gate reads that one token
            // off all four rather than four spellings of the same idea.
            None => "  (nobody recorded as having asked for this run)".to_owned(),
        },
    }
}

/// One run as a person reads it: what it is, who asked for it, and where it got to.
fn render_run(run: &Value) -> String {
    let id = run["id"].as_u64().unwrap_or_default();
    let label = run["label"].as_str().unwrap_or("?");
    let opener = render_who_asked(run);
    // ⛔⛔⛔⛔⛔ **AND WHOSE DECISIONS IT IS BEING JUDGED BY** — register item 870, on the heading
    // beside who asked for it, because the two answer the same shape of question about a run.
    //
    // A kind is not a smaller run, it is a run JUDGED BY ANOTHER document — `debt` names this
    // repository's register by absolute path, so a run under it working in some other tree has its
    // next checkpoint admitted or refused against a record that has never heard of its work.
    // Register item 848 made the caller name one for exactly that reason, and then nothing said
    // which they had named: five live runs, three different kinds between them, rows identical
    // apart from the pane.
    //
    // ⚠ Absent for a plugin that takes no kind, which is most of them — the presence-is-the-claim
    // rule this row's other clauses use.
    let kind = run[sprag_host::plugins::LOOP_KIND_KEY]
        .as_str()
        .map_or_else(String::new, |named| format!("  judged as {named}"));
    let state = &run["state"];
    // ⚠⚠⚠⚠⚠ WHAT BECAME OF A PERSON'S STAND-DOWN — register item 594 — AND WHY IT IS NOT ON THE
    // HEADING AND NOT AT THE END. `render_build`'s doc names the constraint: this repository's own
    // outer-loop watcher (the repayment skill's `watch.sh`) reads the run's STATUS as the line
    // immediately after the heading (`$0 ~ r {getline; print}`) and its walk as the block's LAST
    // line (`… | tail -1`). A clause inserted at either end moves a reader that already exists.
    //
    // So it goes where `stopped` and `failure` already go — a detail clause UNDER the status line,
    // which is the one place in a run's block that is addressed by neither of those two reads.
    let order = run[sprag_host::plugins::RUN_STOOD_DOWN_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⚠⚠⚠ AND WHETHER THIS RUN'S PROMPTS ARE ON THAT PANE AT ALL — register item 591. It goes in
    // the same place and under the same constraint as the clause above: a detail line UNDER the
    // status, where the outer-loop watcher's two positional reads cannot see it.
    //
    // ⚠ The SENTENCE and not the two numbers, `plugins::delivery_sentence`'s own argument: a
    // person who has to compare `delivered` against `folded` by eye is one comparison away from
    // the opposite conclusion, and the whole point is to tell them whether to go and look.
    let prompts = sprag_host::plugins::delivery_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔ AND WHICH REFLECTIONS THOSE FOLDS FELL ON — register item 856(1), in the same place
    // and under the same constraint as the clause above.
    //
    // ⚠ It is a SECOND line rather than a clause on the one above, because the two answer different
    // questions: that one says *can I go and look at that pane*, and this one says *what does
    // folding depend on*. Folding them together would put a diagnosis in the middle of an
    // instruction, and the person reading the instruction is not the person reading the diagnosis.
    let split = sprag_host::plugins::folds_by_reason_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND HOW MANY OF THIS RUN'S PROMPTS BECAME A QUESTION — register item 856, in the
    // same place and under the same constraint as the two clauses above.
    //
    // ⚠ A THIRD line for the second one's reason exactly: `prompts` says *can I go and look at that
    // pane*, `split` says *what does folding depend on*, and this says *how much of what this run
    // typed actually arrived* — the number three separate instruments could not answer, each of
    // them counting every fold and only some landings.
    let landed = sprag_host::plugins::delivered_by_road_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHICH OF THIS RUN'S PROMPTS WERE THE ONES THAT STUCK — register item 889, in
    // the same place and under the same constraint as the three clauses above.
    //
    // ⚠ A FOURTH line, on the third's reason: `landed` says *how much of what this run typed
    // arrived*, and this says *which prompt did not*. They are one number apart and two different
    // acts — the first is a verdict on the run, the second is where to look next, and the measured
    // answer is a fifteen-fold difference between the brief and the turn prompt that no total, no
    // reflection split and no road table can express.
    let stuck = sprag_host::plugins::said_by_sentence_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHAT THE PANE'S WIDTH WOULD HAVE WITHHELD FROM ITS REFLECTION ANSWERS —
    // register item 866(2), in the same place and under the same constraint as the clauses above.
    //
    // ⚠⚠ A LINE OF ITS OWN, and the reason is item 866's whole shape: the four clauses above are
    // about prompts this run SENT, and this is about the answers it READ BACK. Item 866 measured
    // 762 cells written and 161 adopted, every reflection, with `ReflectApplied` publishing
    // success the whole time — so what was silent was never a delivery count. It was the reading.
    //
    // ⚠ AND IT IS HERE RATHER THAN LEFT TO THE JSON, on `authors`' argument one clause down: a key
    // only a program can find is one item 856's arms had to be told apart by a human note.
    let read_back = sprag_host::plugins::width_withheld_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND HOW FULL ITS SESSION GOT AGAINST THE BOUND IT WAS JUDGED BY — register items
    // 894 and 856(1b), in the same place and under the same constraint as the four clauses above.
    //
    // ⚠ A FIFTH line rather than a clause on any of them, on `split`'s reason: those four are
    // about what happened to this run's PROMPTS, and this is about the session receiving them.
    // Item 856 measured that the second is what the first moves with — so a person deciding
    // whether a folded prompt is a pane to walk to or a session to replace needs both lines and
    // needs to be able to tell them apart.
    let fullness = sprag_host::plugins::context_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // 🎯🎯🎯🎯🎯 AND WHICH OF ITS NUMBERS WERE NOT ITS DOCUMENT'S — register item 859(2), in the
    // same place and under the same constraint as the clauses above.
    //
    // ⛔⛔⛔ WHY IT IS HERE AND NOT LEFT TO THE JSON: this key has been on the row since item 853
    // and this renderer never printed it, so a person could learn it existed only by reading the
    // wire or the source. Item 856's arms were told apart from its baseline by a human note for
    // exactly that reason — the answer was published where only a program would find it.
    //
    // ⚠ It rides beside `fullness` deliberately: `context_ceiling` is one of the numbers a caller
    // can take, so a reader who has just been told a run's ceiling needs the next line to say
    // whether that ceiling was the document's at all.
    let authors = sprag_host::plugins::overridden_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⚠⚠⚠ AND WHETHER ANYTHING INDEPENDENT VERIFIED WHAT IT CONVERGED ON — register item 601, in
    // the same place and under the same constraint as the two clauses above.
    let verified = run[sprag_host::plugins::RUN_CHECKS_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHAT THE TREE IT WORKED IN IS HOLDING — register item 682's
    // commit-contamination clause, in the same place and under the same constraint as the clauses
    // above.
    //
    // ⚠⚠ THIS IS THE ONE CLAUSE HERE THAT IS ABOUT WHAT THE READER IS ABOUT TO DO rather than
    // about what the run did. Every other line explains an ending; this one interrupts a COMMIT —
    // a dead run's half-applied edit is still in the tree and shipping it re-introduces whatever
    // it was in the middle of repairing.
    //
    // ⚠ The SENTENCE and not the byte count, `delivery_sentence`'s argument: the number travels in
    // the row for a machine, and a person needs to be told to go and look.
    let uncommitted = sprag_host::plugins::uncommitted_sentence(run)
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔ AND WHAT THE DOOR ACCEPTED — register item 719's second direction, in the same place and
    // under the same constraint as the clauses above.
    //
    // ⚠⚠ THIS IS THE WHOLE OF WHAT *`orchestrate` STOPS ACCEPTING A BRIEF SILENTLY* COMES TO for a
    // person: `orchestrate` answers a run id and points at this row, so this row is where the size
    // it took has to appear. It is a LEVEL and not a journal line for a reason the churning run
    // proves — the journal is bounded, a brief is said once at the start, and the runs this exists
    // for are the long ones that would have evicted it.
    let briefed = run[sprag_host::plugins::RUN_BRIEFED_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⚠⚠⚠⚠ AND WHO RAISED THE CANCEL — register item 596, in the same place and under the same
    // constraint as the three clauses above. This is the one whose absence from the mouth would
    // cost the most: `cancelled` alone is the word item 594 measured a person reading and being
    // unable to act on, and a `Canceller::Shutdown` is only ever read AFTER a restart.
    let canceller = run[sprag_host::plugins::RUN_CANCELLED_BY_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHETHER ANY DAEMON IS GOING TO PICK THIS RUN UP — register item 737, in the same
    // place and under the same constraint as the clauses above.
    //
    // ⚠⚠ THIS IS THE CLAUSE THE WORD `interrupted` MOST NEEDS. That word covers two opposite
    // futures — *waiting to be put back* and *no successor can put it back, because the documents
    // it recorded its position against are not this build's* — and the second is what a PROMOTION
    // causes, which makes it the common one rather than the rare one. A person reading a bare
    // `interrupted` after promoting a build waits for a resume that was decided against.
    let withheld = run[sprag_host::plugins::RUN_WITHHELD_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHAT BECAME OF THE PROCESS THAT WAS STILL DRIVING IT — register item 740, in the
    // same place and under the same constraint as the clause above, and printed straight after it
    // because the two are one answer about one promotion.
    //
    // ⚠⚠⚠⚠ WITHOUT IT THIS ROW IS ITEM 744's CLASS AGAIN, AT THE SAME MOUTH. The daemon has known
    // since item 737 whether something was still typing at that pane, and the fact reached the
    // OPERATOR'S LOG — read by whoever happened to be watching the terminal the daemon was
    // restarted in, which a promotion exists to make unnecessary. The person who comes back to
    // `sprag runs` afterwards is the one who has to decide whether to start the loop again.
    //
    // ⚠⚠ AND IT IS THE CLAUSE THAT SAYS WHY THE WORD ABOVE IS `interrupted` AND NOT `panicked`.
    // Those two were, before item 740, decided by whether a person had reached the driver before
    // the daemon went down — and a watcher of another repository read `panicked` as *my loop hit a
    // bug* and went through its own code looking for one that was not there.
    let leftover = run[sprag_host::plugins::RUN_LEFTOVER_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⛔⛔⛔⛔⛔ AND WHY A BOOT THAT WAS WILLING TO PUT IT BACK COULD NOT — register item 771, in the
    // same place and under the same constraint as the two clauses above, and printed straight after
    // them because all three answer one question: *why is this row still `interrupted`?*
    //
    // ⚠⚠⚠⚠ THE CLAUSE ABOVE COULD NOT REACH THIS RUN, WHICH IS WHY THERE ARE TWO. `withheld` is
    // decided while READING a predecessor's log — are these words this build's? — and a run whose
    // fingerprint matched is withheld from nothing, so that clause is empty for it. The boot then
    // fails on the NEXT step, over a pane, and until this item that failure reached the operator's
    // log and no reader at all. **Measured 2026-08-30**: one promotion, four loops, one identical
    // fingerprint; three came back and the fourth — a loop that had replaced its inner session
    // twice, so the pane its log recorded was gone — read `interrupted` with no clause beside it.
    // The person who found out did it by comparing four log records by hand.
    let not_resumed = run[sprag_host::plugins::RUN_NOT_RESUMED_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!("\n  {said}"));
    // ⚠⚠⚠⚠⚠ AND WHICH PANE A PERSON SHOULD ACTUALLY WALK TO — register item 726, in the same place
    // and under the same constraint as the four clauses above.
    //
    // ⛔⛔ THE NAME ON THE HEADING IS NOT THAT PANE, AND MEASURING IS WHAT SAID SO. Run 18 was named
    // `ai_loop pane=49` and was driving 54: a run REPLACES its inner session as it goes, and
    // `label` is prose composed ONCE, when the run opened. A person following this row's own
    // *go and look at that pane* arrived at a pane that no longer existed — which breaks the one
    // requirement the whole project is built on (items 243/285, a person can always see how the
    // loop is turning), through the mouth that exists to serve it.
    //
    // ⚠⚠⚠ READ OFF THE KEY, AND NEVER BY PARSING THE LABEL. `RUN_DRIVING_KEY` is register item
    // 540's whole point — `Plugin::driving` is asked of the driver on every step and never cached,
    // so it is the live answer where the name is a birth certificate. A clause that cut `49` out of
    // `ai_loop pane=49` in order to COMPARE the two would re-create exactly the derive-it-from-a-
    // name defect that key was built to retire. So this states the live pane outright rather than
    // diffing it against prose, and a reader who sees the two differ has learned the true thing:
    // the run moved, and the row now says where to.
    //
    // ⚠⚠ AND AN ABSENCE IS SAID RATHER THAN FILLED IN — register item 709's discipline, which is
    // the half a reader is most likely to lose. `driving` is absent until a step reports one, so a
    // RUNNING run with no key has no pane anybody has vouched for, and the only number on the row
    // is the name from before any of this happened. Left silent, that name gets read as current —
    // the same failure this clause repairs, one case over. ⚠ A run that has STOPPED says nothing
    // here: its absence is history rather than a question, and a line per finished run would be
    // noise on the rows nobody has to act on.
    let walk_to = match (
        run[sprag_host::plugins::RUN_DRIVING_KEY].as_u64(),
        state["status"].as_str() == Some("running"),
    ) {
        (Some(pane), _) => format!(
            "\n  the pane to walk to is {pane} — the name above was composed when this run opened, \
             and a run that has replaced its session is no longer on it"
        ),
        (None, true) => "\n  ⚠ nothing has reported which pane this run is driving, so the name \
                         above is not an answer to that"
            .to_owned(),
        (None, false) => String::new(),
    };
    // ⛔⛔⛔⛔⛔ WHETHER SOMEBODY IS BEING WAITED ON — register item 755, and **THE ONE CLAUSE HERE
    // THAT GOES ON THE STATUS LINE ITSELF** rather than under it.
    //
    // ⚠⚠⚠⚠⚠ THAT PLACEMENT IS THE WHOLE REPAIR AND IT IS THE OPPOSITE OF EVERY CLAUSE ABOVE. The
    // note on `order` records why those go UNDER the status: this repository's own outer-loop
    // watcher reads the run's status as the line immediately after the heading
    // (`$0 ~ r {getline; print}`), so a detail line is invisible to it — which is exactly what a
    // watcher blind for sixty-two minutes needs to stop being. A clause that only a person reading
    // the whole block can see would repair nothing for the reader that was actually watching.
    //
    // ⚠⚠ APPENDED rather than replacing `running`, so that positional read keeps working and the
    // word every script greps for is still there. What changes is that the line now says the run is
    // going AND that it is going nowhere until somebody comes.
    //
    // ⚠ The daemon's sentence verbatim: which states mean *a person is needed* is the loop
    // document's answer, and a second opinion in this binary would be a copy of a statechart's
    // vocabulary in a client — item 686's class, one fact over.
    //
    // ⚠ Read off `state`, which is where the daemon puts it — and where only a `running` row can
    // have it, because that is the one status whose state object comes from `progress_to_json`.
    // The renderer therefore needs no guard of its own, and a guard here would be a second copy of
    // a rule the wire already keeps.
    let waiting = state[sprag_host::plugins::RUN_WAITING_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!(" · {said}"));
    // ⛔⛔⛔⛔⛔ **AND WHETHER A RESCUED RUN EVER WENT BACK TO WORK** — register item 774, and
    // `waiting`'s neighbour on the status line for `waiting`'s own reason: this repository's
    // outer-loop watcher reads the line after the heading and nothing else, and *the run came back
    // and has been silent for two hours* is exactly the thing that watcher exists to notice.
    //
    // ⚠⚠⚠⚠⚠ **MEASURED, and the numbers are why this is a line rather than a nicety.** One
    // promotion, 2026-08-30: three loops came back and made **zero deliveries between them in two
    // hours**, while four runs started in the same window made one each. Every one of the seven
    // rows said `running`.
    let resumed = resumed_clause(run, state);
    // ⛔⛔⛔⛔⛔ **AND WHETHER THE PANE IT IS TYPING AT CAME BACK FROM A RESTORE** — register item
    // 869, and `waiting`'s neighbour on the STATUS LINE for `waiting`'s own measured reason: the
    // repayment skill's post-promotion check is `sprag runs | grep -E '^run ' -A1`, which is the
    // heading and the one line after it. A clause printed under the status would be invisible to
    // the exact reading this item exists to serve.
    //
    // ⚠⚠⚠⚠⚠ **AND THE CHECK IT REPAIRS IS A PROMOTION'S, WHICH IS WHY IT COULD NOT BE A RULE.**
    // The skill already said *if the pane carries `--resume`, kill it and open a fresh one* — in
    // its section 3a, *before using a pane*. A promotion does not USE a pane, it MAKES one, so that
    // rule was never on the path: measured over four promotions, every inner pane in every
    // repository came back resumed and nothing anybody typed afterwards asked.
    //
    // ⚠ The daemon's sentence verbatim, `waiting`'s rule: which panes were revived is the host's
    // answer, and a second opinion composed here would be a client re-deciding a fact only the
    // daemon's boot can know.
    //
    // ⚠ Read off `state`, where only a `running` row can carry it — the same structural guard
    // `waiting` above relies on, so the renderer needs no status test of its own.
    let revived_pane = state[sprag_host::plugins::RUN_REVIVED_PANE_KEY]
        .as_str()
        .map_or_else(String::new, |said| format!(" · {said}"));
    let head = format!(
        "run {id}  {label}{opener}{kind}{}{}{}\n",
        render_build(run),
        // ⛔⛔⛔⛔⛔ AND WHICH REPOSITORY — register item 890, on `render_which_run`'s measured
        // constraint: the repayment skill's `watch.sh` anchors on `^run <N> ` as a PREFIX and takes
        // a run's status from the line after the heading, so a clause inside the heading moves
        // neither.
        //
        // ⛔⛔⛔⛔⛔ **BEFORE THE STAMP AND NEVER AFTER IT, AND THAT ORDER IS LOAD-BEARING.** The
        // first build of this clause appended it at the end with a comment claiming that reader's
        // pattern *"still takes the stamp — this clause carries no brackets"*. It does not. The
        // expression is `sed -n 's/.*\[\([^][]*\)\]$/\1/p'` — **anchored at `$`** — so a head line
        // ending in anything but `]` yields NOTHING, the watcher falls back to *which run not
        // recorded*, and item 887's whole point (a watcher can tell a reissued number from its own
        // run) is silently off. Measured by running that exact expression over both orders:
        // stamp-last answers `1f4a-…`, tree-last answers the empty string.
        //
        // ⇒ ⭐ The rule this is an instance of: a clause added to a line other readers parse is
        // safe at the END only if nothing else is anchored there. Here something was.
        render_tree(run),
        render_which_run(run),
    );
    match state["status"].as_str() {
        // ⚠ THE COUNTERS, so a person watching a long loop can tell PROGRESS from STUCK — two looks
        // showing the same numbers is the answer to that question, and `running` alone was not.
        Some("running") => format!(
            "{head}  running — {} iterations, {} {} so far{waiting}{resumed}{revived_pane}{}{}{}{order}{walk_to}{briefed}{prompts}{split}{landed}{stuck}{read_back}{fullness}{authors}{verified}{canceller}\n{}",
            state["iterations"].as_u64().unwrap_or_default(),
            state["cost"].as_u64().unwrap_or_default(),
            state["unit"].as_str().unwrap_or("steps"),
            // ⚠⚠ WHILE IT IS STILL RUNNING, which is the whole point of putting it here: an
            // approval a person only learns about once the loop is over is one they could not have
            // stopped.
            render_answered(state),
            // 🎯🎯🎯 AND WHAT IT HAS SET ASIDE, on exactly that argument — register item 833(2). A
            // cap too tight for the work shows up as this number climbing, and the person who can
            // widen the brief is the one reading this row now.
            render_deferred(state),
            // 🎯🎯🎯 AND HOW MANY OF THOSE DIRECTIONS NOBODY CHECKED — register item 847, live for
            // the line above's reason: the person watching is the one who can go and name a
            // classifier for this kind before the run takes another.
            render_unchecked(state),
            render_journal(run),
        ),
        Some("done") => {
            let outcome = &state["outcome"];
            let unit = outcome["unit"].as_str().unwrap_or("steps");
            // ⛔⛔⛔⛔⛔ AND WHICH ENDING IT CLOSED UNDER — register item 706's third requirement, in
            // the same place and under the same constraint as the clauses above.
            //
            // ⚠⚠⚠⚠ WITHOUT IT THIS ROW COLLAPSES ITEM 594 AGAIN, AT THE ONE MOUTH A PERSON HAS.
            // All three endings an `ai_loop` closes under publish `converged`, and `runs` has no
            // machine form — so the word that separates them reached an agent through `list_runs`
            // and reached a person nowhere. `stood_down`'s sentence covers ONE of the three and
            // only when somebody gave an order, which leaves *the agent declared the north star
            // reached* and *the reflection named no successor* byte-identical here.
            //
            // ⚠⚠⚠ THE WORD IS THE PLUGIN'S AND THIS MOUTH FRAMES IT WITHOUT SPELLING IT. A match
            // over `declared` / `no_successor` / `stood_down` would make this renderer a second
            // authority on a set `sprag_plugin` owns — `RUN_DONE_REASON_KEY`'s doc refuses exactly
            // that — and it would fall silent on a fourth ending instead of carrying it. So the
            // word travels verbatim and the sentence claims only what the host can know: that the
            // state word above does not answer this question.
            let closed_under = outcome[sprag_host::plugins::RUN_DONE_REASON_KEY]
                .as_str()
                .map_or_else(String::new, |word| {
                    format!(
                        "\n  it closed under `{word}` — the ending its own plugin named, which `{}` \
                         above does not say",
                        outcome["state"].as_str().unwrap_or("?"),
                    )
                });
            // ⛔⛔⛔⛔⛔ AND WHY A BLOCKED RUN WAS NEVER ANSWERED — register item 903, beside
            // `closed_under` because it is the same question one ending over. `blocked` says
            // SOMEBODY HAS TO ANSWER THIS and, until this clause, said nothing about what stopped
            // this host answering: measured 2026-09-05T05:05:23Z, 14 blocked runs and 0 carrying a
            // reason a reader could see.
            //
            // ⚠⚠ IT IS HERE RATHER THAN LEFT TO THE JSON, on the clause below's standing argument:
            // a fact that reaches the wire and dies at the mouth somebody actually reads is the
            // shape this file keeps paying for, and item 903 is that shape at its largest.
            let blocked_on = outcome[sprag_host::plugins::RUN_BLOCKED_BY_KEY]
                .as_str()
                .map_or_else(String::new, |why| {
                    format!("\n  nothing answered it: `{why}`")
                });
            let output = state["output"]
                .as_str()
                .map_or_else(String::new, |text| format!("  ---\n{text}\n"));
            // 🎯🎯🎯🎯🎯 AND WHAT HAPPENS NEXT — register item 827, in the same place and under the
            // same constraint as the clauses above.
            //
            // ⛔⛔⛔ THE MEASUREMENT. Item 798 paid *the ending reaches a person* and said in its
            // own body that it was not paying *what somebody then does about it*. On 2026-09-02 a
            // sprag run ended at 08:10:18 and the next driver started **three hours forty-nine
            // minutes** later, while two other repositories were re-launched inside three minutes.
            // The ending was on a screen the whole time. What was missing was not reach — 798
            // bought that — it was an answer to *and now what*, which lived in three of the six
            // outcomes' DOC COMMENTS and nowhere a reader could ask.
            //
            // ⚠⚠ ASKED OF THE TYPE, never spelled here — `outcome_word`'s own rule, and the reason
            // this clause is three lines rather than a `match` on words. `Disposition` walks
            // `OutcomeState::EVERY_SHAPE`, so the classification lives in exactly one place; a
            // second opinion in this binary is item 686's class and items 855 and 864's defect.
            //
            // ⚠⚠⚠ AND AN UNCLASSIFIED ENDING IS SAID, NOT SKIPPED. A word no disposition covers is
            // a seventh outcome that reached the wire without anybody deciding what to do about it
            // — which is precisely the state item 827 was filed on. Silence here would render that
            // state as *nothing to do*, inventing the answer the item says must be recorded.
            let disposition = outcome["state"].as_str().map_or_else(String::new, |word| {
                sprag_plugin::driver::Disposition::of_outcome_word(word).map_or_else(
                    || {
                        format!(
                            "\n  ⚠ NOTHING HAS CLASSIFIED what happens next after an ending \
                             spelled {word:?}, so this row cannot tell you — register item 827"
                        )
                    },
                    |next| format!("\n  {}", next.describe()),
                )
            });
            format!(
                "{head}  {}{} after {} iterations, {} {unit}{}{}{closed_under}{blocked_on}{disposition}{order}{walk_to}{briefed}{prompts}{split}{landed}{stuck}{read_back}{fullness}{authors}{verified}{uncommitted}{canceller}{}{}{}{}\n{}{output}",
                outcome["state"].as_str().unwrap_or("?"),
                // ⚠ WHICH CEILING stopped it — the same fact the agent's renderer prints, for the
                // same reason: `exhausted` names a class of ending and not the bound to change.
                outcome[sprag_host::plugins::RUN_CEILING_KEY]
                    .as_str()
                    .map_or_else(String::new, |ceiling| format!(" ({ceiling})")),
                outcome["iterations"].as_u64().unwrap_or_default(),
                outcome["cost"].as_u64().unwrap_or_default(),
                outcome["failure"]
                    .as_str()
                    .map_or_else(String::new, |why| format!("\n  failed: {why}")),
                // ⚠⚠ AND WHAT BECAME OF THE WORK, present only for a run that was CUT SHORT. A
                // person who cancelled a loop and was told only `cancelled` cannot tell whether the
                // peer stopped or is still spending — and `Stopped::Unreached` and
                // `Stopped::Unsupported` both say it is still going. ⚠ The two are NAMED rather
                // than counted, register item 872(2)'s lesson: a tally in prose is a number
                // nothing reads and it goes stale the moment an arm is added. Dropping this
                // clause was the shape this project keeps paying for: a fact that reaches the
                // wire and dies at the mouth somebody actually reads.
                outcome[sprag_host::plugins::RUN_STOPPED_KEY]
                    .as_str()
                    .map_or_else(String::new, |stopped| format!("\n  {stopped}")),
                render_answered(outcome),
                // 🎯🎯🎯🎯🎯 AND WHAT IT SET ASIDE AT ITS DEPTH CAP — register item 833(2). It is
                // on the ENDING as well as on the live row because the question it answers is asked
                // most often about a run that is already over: *did this run go where I pointed it,
                // and what did it leave behind on the way?* The count without a mouth is the
                // failure this file names in four other places.
                render_deferred(outcome),
                // 🎯🎯🎯 AND HOW MANY OF ITS DIRECTIONS NOBODY CHECKED — register item 847, on the
                // ENDING as well for the line above's reason: the question is asked most often
                // about a run that is already over, and *did anything bound where this went?* is
                // the first thing a reader of such an account needs settled.
                render_unchecked(outcome),
                // ⚠⚠⚠ AND WHAT THE PEER IS ASKING. A `blocked` run is one that WANTS AN ANSWER from
                // the person reading this, and the word alone sent them to go find the pane and
                // parse a menu this daemon had already parsed for them.
                render_asking(outcome),
                render_journal(run),
            )
        }
        // ⚠⚠ `interrupted` AND `panicked` COME THROUGH HERE, and item 594 was MEASURED on the
        // first of them: a daemon restarted under a standing order left a person a bare word and
        // no way to learn that what they asked for had never happened.
        //
        // ⛔⛔⛔⛔⛔ AND ITEM 685 IS THE SECOND OF THEM: `panicked` reaches the wire carrying WHY —
        // `RunState::Panicked` holds the sentence `driver_ending` composed, which names the exit
        // status and so names the SIGNAL when one killed the driver — and this arm printed the word
        // alone. A watcher of another repository read `panicked` as *my run hit a bug* when what
        // had happened was a `kill-server`. **A fact that reaches the wire and dies at the mouth
        // somebody actually reads** is the sentence the `Reported` arm above already wrote down.
        _ => format!(
            "{head}  {}{}{withheld}{leftover}{not_resumed}{order}{prompts}{split}{landed}{stuck}{read_back}{fullness}{authors}{verified}{canceller}\n",
            state["status"].as_str().unwrap_or("?"),
            render_why_it_ended(state),
        ),
    }
}

/// Park until run `id` leaves `running`, then answer its entry.
///
/// ⚠ NO DEADLINE OF ITS OWN, deliberately — and the run now HAS one, which makes the argument
/// stronger rather than obsolete. A run is bounded by ITS guardrails: the iteration ceiling, the
/// cost ceiling, and the wall-clock deadline it was started with. A waiter that gave up early would
/// be inventing a second, weaker bound and reporting a run as unfinished that the daemon is still
/// correctly running — and it would now be a duplicate of a bound the run already carries. The
/// bound belongs to the run; the wait belongs to the person, who has a keyboard.
fn wait_for_run(conn: &mut HostConn, session: Option<&str>, id: u64) -> io::Result<Value> {
    loop {
        let answer = query_slot(
            conn,
            scoped_params(
                session,
                sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
            ),
        )?;
        let entry = answer
            .as_array()
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run["id"].as_u64() == Some(id))
                    .cloned()
            })
            .ok_or_else(|| {
                bad_input(&format!(
                    "orchestrate --{WAIT_FLAG}: run {id} is gone from the daemon"
                ))
            })?;
        if entry["state"]["status"].as_str() != Some("running") {
            return Ok(entry);
        }
        std::thread::sleep(RUN_POLL);
    }
}

/// `cancel-run ID`: ask a run to stop at its next step.
///
/// The flag the run's own `RunContext` polls, which every wait inside the driver checks — so a
/// cancel lands between steps rather than killing a thread mid-write.
fn cancel_run(args: Vec<String>) -> io::Result<()> {
    let (session, args) = scope_and_rest(args, "cancel-run")?;
    let mut rest = args.into_iter();
    let id = rest
        .next()
        .ok_or_else(|| bad_input("cancel-run: which run? (see `sprag runs`)"))?;
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "cancel-run: unexpected argument {extra:?} (one run at a time)"
        )));
    }
    let id: u64 = id.parse().map_err(|_| {
        bad_input(&format!(
            "cancel-run: {id:?} is not a run id (see `sprag runs`)"
        ))
    })?;
    // Pre-flighted, and this verb is why the family's own defect mattered — see [`runs`].
    let mut conn = connect_scoped(session.as_deref())?;
    invoke_action(
        &mut conn,
        scoped_call(
            session.as_deref(),
            sprag_host::wire::plugins_path(sprag_host::plugins::CANCEL_ACTION),
            json!({ "id": id }),
        ),
    )?;
    println!("run {id} asked to stop; `sprag runs` says when it has");
    Ok(())
}

/// `stand-down ID`: ask a run to finish what it is doing and then stop.
///
/// # ⚠⚠⚠ Why this is not `cancel-run` with a flag
///
/// A cancel stops a loop MID-TURN: whatever the agent had written since its last checkpoint is
/// thrown away, and the run reports `cancelled`. This one waits — the milestone the agent is working
/// toward is finished, `closing` takes its account, and the run **converges**. The work is banked.
///
/// ⛔⛔⛔ **THIS PARAGRAPH USED TO SAY THE RUN CONVERGES *"reporting `stood_down`"*, AND THAT WORD
/// REACHES NO READER OF `sprag runs`** — register item 594. `stood_down` is
/// [`sprag_plugin::DoneReason::StoodDown`]'s word, which the loop DOCUMENT assigns and which is
/// rendered into a walk and nowhere else; what this command's own second sentence sends a person to
/// look at publishes an `OutcomeState`, and that vocabulary has no such word. The repayment skill
/// then copied it into a table defining `stood_down` as a run state, so a run reported `cancelled`
/// read as the opposite of a promise rather than as a promise nobody could check.
/// `sprag_plugin::outer::tests::the_promise_about_a_stand_down_names_the_word_a_stood_down_run_reports`
/// is what stops it being written again — it drives a stood-down run and holds the sentence to the
/// word that run really ends with.
///
/// Those are opposite outcomes from one keystroke's distance apart, which is exactly why they are
/// two verbs: a mode flag would let somebody lose a milestone by mistyping a boolean at the end of a
/// long day, which is when this verb is most likely to be typed at all.
///
/// ⚠ It returns as soon as the order is recorded. The run reads it at its next pass and acts on it
/// at its next MILESTONE, which may be many minutes of a real agent away — `sprag runs` is what says
/// it has landed.
fn stand_down(args: Vec<String>) -> io::Result<()> {
    let (session, args) = scope_and_rest(args, "stand-down")?;
    let mut rest = args.into_iter();
    let id = rest
        .next()
        .ok_or_else(|| bad_input("stand-down: which run? (see `sprag runs`)"))?;
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "stand-down: unexpected argument {extra:?} (one run at a time)"
        )));
    }
    let id: u64 = id.parse().map_err(|_| {
        bad_input(&format!(
            "stand-down: {id:?} is not a run id (see `sprag runs`)"
        ))
    })?;
    // Pre-flighted for [`cancel_run`]'s reason, which is [`runs`]'.
    let mut conn = connect_scoped(session.as_deref())?;
    // ⛔⛔⛔⛔⛔ AND WHERE THIS ORDER IS COMING FROM — register item 835, through the SAME door
    // `orchestrate` records a run's opener with ([`asking_pane`]): the caller points at its own
    // pane, a stale `$SPRAG_PANE` is dropped with a word on stderr rather than refusing the order,
    // and the DAEMON reads the conversation off that pane itself.
    //
    // ⚠⚠⚠ **THIS COMMAND IS THE ONE THE MEASUREMENT WAS TAKEN ON.** Item 835 is another
    // repository's watcher reading *"a person asked this run to stand down"* about an order given
    // from a session exactly like this one, and re-launching the run twice because *person* named
    // nobody it could ask. A supervisor standing runs down from its own pane is the common case,
    // not the exotic one.
    //
    // ⚠ Absent when this process is not in a pane at all — a person at a plain terminal — and that
    // absence is published as *nobody wrote it down* rather than as *a person*. See
    // `crate::runs::StoodDownBy::UNRECORDED`.
    let mut call = json!({ "id": id });
    if let Some(pane) = asking_pane(&mut conn, session.as_deref()) {
        call[sprag_host::plugins::STOOD_DOWN_BY_KEY] = json!(pane);
    }
    invoke_action(
        &mut conn,
        scoped_call(
            session.as_deref(),
            sprag_host::wire::plugins_path(sprag_host::plugins::STAND_DOWN_ACTION),
            call,
        ),
    )?;
    // ⚠⚠⚠ THE SENTENCE IS THE PRODUCT'S, NOT THIS COMMAND'S — register item 594, and register item
    // 522's remedy applied to the order beside `hold`. The words live in the crate that holds the
    // document that keeps them (`sprag_plugin::STAND_DOWN_TAKES_EFFECT`), where a gate can drive a
    // real stood-down run and hold the promise to the ending it actually reaches. Prose here could
    // not be turned red by anything, and for a whole round it was not: it named an outcome word
    // that reaches no reader of `sprag runs`.
    println!(
        "run {id} asked to finish up; {}",
        sprag_plugin::STAND_DOWN_TAKES_EFFECT,
    );
    Ok(())
}

/// `hold-run ID` / `resume-run ID`: **HALT A RUN BETWEEN TURNS, AND LET IT GO AGAIN** — register
/// item 9, and the third thing a person may say to a run.
///
/// The other two both END it: `cancel-run` throws the turn in flight away, `stand-down` banks the
/// milestone and converges. Neither of them is *wait, let me read this pane* — and `ai_loop.scxml`
/// has carried the edge for it since R378 with nothing able to raise it.
///
/// ⚠ ONE FUNCTION FOR BOTH DIRECTIONS, because they are one order with a sign. Two bodies would be
/// two places for the id-parsing and the sentence to drift apart, and the wire verb takes the
/// direction as an argument for exactly this reason.
///
/// ⚠⚠ **AND THE SENTENCE IS CHOSEN BY WHAT THE DAEMON FOUND, NEVER BY THE DIRECTION ASKED FOR** —
/// register item 694. `held` is what this caller WANTS; the level the order arrived at is a fact
/// only the registry holds, and reading the first as the second is what made `resume-run` promise
/// a fresh turn to runs nobody was holding. The four pairings are
/// `sprag_host::plugins::hold_sentence`'s to spell.
fn hold_run(args: Vec<String>, held: bool) -> io::Result<()> {
    let verb = if held { "hold-run" } else { "resume-run" };
    let (session, args) = scope_and_rest(args, verb)?;
    let mut rest = args.into_iter();
    let id = rest
        .next()
        .ok_or_else(|| bad_input(&format!("{verb}: which run? (see `sprag runs`)")))?;
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "{verb}: unexpected argument {extra:?} (one run at a time)"
        )));
    }
    let id: u64 = id.parse().map_err(|_| {
        bad_input(&format!(
            "{verb}: {id:?} is not a run id (see `sprag runs`)"
        ))
    })?;
    // Pre-flighted for [`cancel_run`]'s reason, which is [`runs`]'.
    let mut conn = connect_scoped(session.as_deref())?;
    let answer = invoke_action(
        &mut conn,
        scoped_call(
            session.as_deref(),
            sprag_host::wire::plugins_path(sprag_host::plugins::HOLD_RUN_ACTION),
            json!({ "id": id, "held": held }),
        ),
    )?;
    // ⚠ THE SENTENCE SAYS WHAT IS AND IS NOT TRUE YET. A hold takes effect at the run's next pass,
    // not at this return — and a person who read "held" and started editing the pane mid-turn would
    // be typing underneath an agent that is still working.
    //
    // ⛔⛔⛔ AND THE WORDS ARE NOT THIS COMMAND'S — register item 522. What stood here said the run
    // *"stops at its next turn boundary and waits"*, where `ai_loop.scxml` parks it on the very next
    // pass, mid-turn; a person told to expect a boundary waits for one. The sentence now belongs to
    // the crate that holds the document that does it ([`sprag_plugin::HOLD_TAKES_EFFECT`]), where a
    // gate reads BOTH and neither can move alone. This command holds no wording of its own to drift.
    //
    // ⛔⛔⛔⛔⛔ AND NEITHER IS THE DIRECTION THE SENTENCE IS CHOSEN BY — register item 694. What
    // stood here branched on `held`, which is what this command ASKED FOR, so `resume-run` said
    // *"let go"* over runs nobody was holding: the word a person acts on was picked from their own
    // request rather than from anything the daemon found. The daemon answers the level now, and the
    // four pairings of *found* against *left* are `hold_sentence`'s to spell.
    match answer
        .as_str()
        .and_then(sprag_host::runs::Holding::from_wire)
    {
        Some(holding) => println!(
            "{}",
            sprag_host::plugins::hold_sentence(sprag_host::runs::RunId(id), holding)
        ),
        // ⚠⚠⚠ A DAEMON OLDER THAN THIS ANSWER SAYS SO, and does not get a sentence composed on its
        // behalf. It took the order — the call returned — and the level it found is the one thing
        // it cannot tell us; guessing from `held` here would rebuild the exact defect above, and
        // this is the surface where client and daemon skew is ordinary rather than hypothetical.
        None => println!(
            "run {id} was sent the {verb} order and this daemon took it, but it does not say what \
             it found — it is older than this command, so whether the level actually moved cannot \
             be stated here.",
        ),
    }
    Ok(())
}

/// The published forms of one plugin-host verb, asked of the daemon this connection holds.
fn published_forms(
    conn: &mut HostConn,
    session: Option<&str>,
    action: &str,
) -> io::Result<Vec<PublishedForm>> {
    let answer = query_slot(
        conn,
        scoped_params(session, sprag_host::wire::plugins_path(ACTION_GRAMMAR_SLOT)),
    )?;
    let table = answer.get(action).ok_or_else(|| {
        bad_input(&format!(
            "this daemon publishes no grammar for {action:?}, so there is nothing to build a call \
             from — it is older than this command."
        ))
    })?;
    PublishedForm::read_all(table, action).map_err(|error| bad_input(&format!("{error}")))
}

/// How long the CLI asks the daemon to measure the competition over.
///
/// A constant and not a flag: the wire takes a window because it MUST be a parametric address (a
/// bare slot is read by every whole-surface snapshot, and this is the one read that sleeps), and
/// nobody has asked to vary it. A flag whose value no caller chooses is an answer nobody reads.
const DOCTOR_WINDOW_MS: u64 = DOCTOR_WINDOW.as_millis() as u64;

/// The one-glyph-free mark each verdict prints under.
///
/// Words rather than symbols: this output is read in a terminal that may be any width and piped
/// into anything, and `grep -c DEGRADED` is a thing a person will do with it.
fn verdict_mark(verdict: Verdict) -> String {
    match verdict {
        Verdict::Healthy => "  ok    ".to_owned(),
        Verdict::Degraded => "DEGRADED".to_owned(),
        // The reason, not a blank: "this was fine" and "nobody could look" send a reader in
        // opposite directions, which is the distinction the whole report is built on.
        Verdict::Blind(reason) => format!("  --      ({reason})"),
    }
}

/// How many processes the pane holds.
fn count(processes: Counted) -> String {
    match processes {
        Counted::Now(1) => "1 process".to_owned(),
        Counted::Now(many) => format!("{many} processes"),
        Counted::NoController => "(no pids controller)".to_owned(),
    }
}

/// A usage joined to the ceiling it is measured against — `6 MiB of 512 MiB`, or the usage alone
/// where there is no ceiling to measure it against.
///
/// # Why the ceiling is folded into the usage column rather than given one of its own
///
/// It is not an independent fact, it is the DENOMINATOR: `6 MiB` tells a person nothing until they
/// know whether the pane may reach 8 MiB or 8 GiB. A separate column would let a reader see one
/// without the other, which is the same mistake as printing cores held without time spent waiting.
///
/// The two absences print differently and both are silent about the ceiling on purpose: an
/// [`Ceiling::Uncapped`] pane has a real number and no bound, and a
/// [`Ceiling::NoController`] pane's usage column has already
/// said `(no memory controller)` — appending a second sentence saying the same controller is missing
/// would be this surface disagreeing with nobody at twice the width.
fn of(usage: String, ceiling: Ceiling, spell: fn(u64) -> String) -> String {
    match ceiling {
        Ceiling::At(most) => format!("{usage} of {}", spell(most)),
        Ceiling::Uncapped | Ceiling::NoController => usage,
    }
}

/// A memory CEILING as a person reads it — the same units [`footprint`] uses, so a usage and its
/// bound in one phrase are in one scale.
fn footprint_ceiling(bytes: u64) -> String {
    footprint(Counted::Now(bytes))
}

/// A process CEILING as a person reads it — bare, because [`of`] has already printed the noun.
fn count_ceiling(most: u64) -> String {
    most.to_string()
}

/// The share of its level a pane is granted, as the kernel is holding it.
///
/// **Never rendered as a predicted share of the machine**, which is the rule the design behind this
/// feature states and measured twice: a nominal 10:100 came out at 18:82 because the kernel
/// distributes weight per runqueue, and a cgroup weighted 10 took all 8 cores it was offered when
/// its sibling went idle. So the number is printed as the setting it is, beside the cores actually
/// held — which is the only honest pairing.
fn weight(share: Counted) -> String {
    match share {
        Counted::Now(weight) => weight.to_string(),
        Counted::NoController => "(no cpu controller)".to_owned(),
    }
}

/// WHETHER THE REPORTER THAT PRODUCED A VERDICT IS THIS DAEMON'S OWN IMAGE — the sentence
/// [`agent`] prints under a REPORTED state, beside the one that says whether that reporter can
/// still speak (register item 473).
///
/// # ⚠⚠⚠⚠⚠ The quiet half of the hazard is the one that had no voice
///
/// Two things can be wrong with a reporter. The LOUD one — it has stopped being able to deliver —
/// already has its sentence on this surface (*"⚠ THAT REPORTER IS MUTE"*, register item 344). The
/// QUIET one is the worse of the pair and is register item 412: **the numbers agree, the reports
/// are accepted, and the reporter is running code the daemon has never seen.** A `cargo build`
/// replaces the hook binary under every running daemon at once, so that skew is the ORDINARY state
/// after a rebuild rather than an exotic one — and until this it was legible to a wire client alone,
/// because the fact reached the pane row and no surface a person reads rendered it.
///
/// # Why the comparison is made HERE and not by the reporter
///
/// [`sprag_host::wire::AGENT_BUILD_KEY`] divides the work the way `source` divides it: the reporter
/// STATES what it is, and the holder of both halves compares. The reporter cannot — it knows one
/// connection's answer where the daemon has to answer for every reporter it holds — so it sends the
/// raw fact and this renders the judgement.
///
/// ⚠⚠ **`daemon` is the DAEMON's build, never this client's.** The client is rebuilt every round and
/// the daemon is not, so comparing the reporter against the tree that just built `sprag` would
/// answer a question nobody asked and call it this one. [`doctor`] owns the client/daemon pair
/// ([`build_report`]); this owns the reporter/daemon pair.
///
/// # ⚠⚠⚠ Four answers, and a surface that renders three has re-introduced the defect
///
/// The reporter's three are the key's own: it is this image, it is NOT, or it did not say. `None`
/// is *it did not say* and never *it matches* — the rule [`sprag_rpc::BUILD_FIELD`]'s no-bump
/// argument rests on, and the exact inversion the key exists to end.
///
/// The fourth belongs to the OTHER half of the comparison: a daemon predating that field answers no
/// build of its own, which makes the comparison IMPOSSIBLE rather than successful. It gets words
/// for the same reason — an absence that renders as agreement is this client inventing an answer
/// nobody gave it.
///
/// Pure, and takes both builds rather than a connection, so every answer is gateable — including
/// the two a live daemon on this machine cannot be made to produce.
///
/// ⚠⚠⚠ **THE COUNTING IS NOT DONE HERE.** [`sprag_host::wire::reporter_image`] decides which of the
/// four is true and this writes a PERSON's sentence for it; `sprag-mcp` writes an agent's from the
/// same arm (register item 474). One vocabulary, two mouths — and a mouth that came to count
/// differently is the defect the shared arm makes unrepresentable rather than merely unlikely.
fn reporter_build_report(reporter: Option<&str>, daemon: Option<&str>) -> String {
    match sprag_host::wire::reporter_image(reporter, daemon) {
        sprag_host::wire::ReporterImage::SameImage { build } => format!(
            "    that reporter is this daemon's own image (build {build}), so the state above was \
             produced by the code this daemon is running."
        ),
        // ⚠ THE ONE A READER MUST ACT ON: the verdict above outranks the screen and was produced by
        // code this daemon has never run. Both builds are named — a skew that names one tells a
        // reader nothing about which is which.
        sprag_host::wire::ReporterImage::OtherImage { reporter, daemon } => format!(
            "    ⚠ THAT REPORTER IS NOT THIS DAEMON'S IMAGE: it is build {reporter} and this \
             daemon is build {daemon}. The state above was produced by code this daemon has never \
             run — the ordinary state after a `cargo build` replaced the hook under it. Restart \
             the daemon (`sprag kill-server`, then start it again) to make the two one image."
        ),
        // ⚠ Not a match and not a mismatch: nobody can compare. A daemon predating the hello's
        // build key is the only way here, and printing agreement would be this client inventing an
        // answer that daemon could not give.
        sprag_host::wire::ReporterImage::DaemonSilent { reporter } => format!(
            "    that reporter is build {reporter}, and this daemon does not say which build IT \
             is, so the two cannot be compared — an absent build is not a matching one."
        ),
        // ⚠⚠ AND THE ARM THAT MUST NOT COLLAPSE INTO THE FIRST. Every reporter older than
        // `AGENT_BUILD_KEY` answers exactly this, and silence here would read as agreement.
        sprag_host::wire::ReporterImage::ReporterSilent => {
            "    that reporter did not say which build it is, which is not the same as saying it \
             matches — whether it is this daemon's image is unknown."
                .to_owned()
        }
    }
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
/// asked about that pane, which is [`resolve_pane`]'s rule, and "no manifest claims this" is a real
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
    let mut asked: Option<String> = None;
    for arg in rest {
        if asked.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("agent: unexpected argument {arg:?} (agent [-t SESSION] [PANE])"),
            ));
        }
        asked = Some(arg);
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_optional_pane(&mut conn, session.as_deref(), asked.as_deref(), "agent")?;
    let wanted = site.as_ref().map(|site| site.id);
    // FIRST, and on stderr. Every line below was produced by a ruleset that is not the user's, so
    // the caveat has to arrive before the readings it qualifies — and a script slicing `ID: STATE`
    // out of stdout must not have to skip a sentence to find them.
    let manifests: Value = query_slot(
        &mut conn,
        scoped_params(session.as_deref(), mux_action_path(AGENT_MANIFESTS_SLOT)),
    )?;
    if let Some(error) = manifests["error"].as_str() {
        eprintln!("sprag: agent: {error}");
        eprintln!(
            "sprag: agent: the states below came from the manifests that last worked, not from \
             that file"
        );
    }
    // WHICH BUILD THE DAEMON ANSWERING THIS IS, taken once and before the rows it qualifies — the
    // other half of every reporter's `build` below ([`reporter_build_report`]). Read off the
    // CONNECTION rather than from `sprag_host::wire::BUILD`, because this binary is not the daemon:
    // the client is rebuilt every round and the daemon is whatever was started.
    let daemon_build = conn.daemon_build().map(str::to_owned);
    // ⚠⚠⚠⚠ AND WHICH RUN OF IT, taken at the same moment and for a different question — register
    // item 711. The build dates the CODE behind these rows; the generation dates their NUMBERS, and
    // the mute breadcrumb below is filed under a number this daemon's counter reissued from one. A
    // breadcrumb from a daemon that is gone was read as a live mute here on 2026-08-26 and a healthy
    // reporter was taken off its hook.
    let daemon_generation = conn.daemon_generation().map(str::to_owned);
    // ⚠ THE DIRECTORY AND THE GENERATION TRAVEL TOGETHER (`hooks::MuteReader`), because a reader
    // holding one of them judges an old breadcrumb wrongly in a different way for each: the wrong
    // directory answers about another host's history (item 700) and the missing generation answers
    // about another daemon's (item 711).
    let state_dir = sprag_host::durability::state_dir();
    let mute = sprag_host::hooks::MuteReader::new(&state_dir, daemon_generation.as_deref());
    // The listing is read in the RESOLVED pane's window, so `sprag agent buildout` answers about a
    // pane one window over — where a sibling agent most often is, since an agent's work pane and a
    // person's reading pane are why a session has more than one window.
    let listed: Value = query_slot(
        &mut conn,
        match &site {
            Some(site) => site_params(session.as_deref(), site, mux_action_path(PANES_SLOT)),
            None => scoped_params(session.as_deref(), mux_action_path(PANES_SLOT)),
        },
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
        // ⚠⚠ AND THE QUESTION COUNT BESIDE THE STATE COUNT — register item 441. A reader watching a
        // supervised pane needs to tell *the verdict moved* from *the peer took a new question*, and
        // until this was printed the second could only be inferred from the first, which is exactly
        // the inference that was wrong.
        let asked_seq = agent["asked_seq"].as_u64().unwrap_or(0);
        // ⚠⚠⚠ AND THE ANSWER COUNT BESIDE IT, because the two together are what say where a turn
        // stands: `asked` moved and `said` did not is a peer still working on the question, and
        // both moved is a turn that ended. Reading the first alone is what made a whole round's
        // live measurement ambiguous.
        let said_seq = agent["said_seq"].as_u64().unwrap_or(0);
        println!("{id}: {state}  {name}  {origin}  seq={seq}  asked={asked_seq}  said={said_seq}");
        if wanted.is_some() {
            // ⚠⚠⚠⚠⚠ **THE REPORTER'S HEALTH IS PRINTED BEFORE THE AUTHORITY BRANCH, AND THAT MOVE
            // IS REGISTER ITEM 709's** — it used to sit INSIDE the `Some(source)` arm, which was
            // right for as long as a mute reporter went on answering for its pane.
            //
            // It does not any more: a reporter that has left word it cannot deliver no longer
            // outranks the screen (`sprag_detect::Tracker::set_reporter_mute`), so the row above
            // carries a `rule` and no `source` — and the sentence explaining WHY would have
            // disappeared at exactly the moment it became the whole answer. A person would then read
            // *`working-footer` is the rule that fired* about a pane whose hook is broken, with
            // nothing anywhere saying so.
            //
            // ⚠ Read off the filesystem rather than asked of the daemon ON PURPOSE: the condition
            // being reported is precisely that the hook could not reach the daemon, so the daemon is
            // the one party that cannot learn it from a report.
            //
            // ⚠⚠⚠⚠ AND ATTRIBUTED BEFORE IT IS ACTED ON — register item 711. This used to print on
            // the file's mere existence, and the file is keyed on a pane NUMBER that the next
            // daemon's counter reissues: on 2026-08-26 a breadcrumb from 14:02 was printed against a
            // live pane 4 whose child had started at 22:57, and a watcher believing it called
            // `release-agent` on a healthy reporter.
            let word = mute.word_from(id);
            match &word {
                sprag_host::MuteWord::Speaking => {}
                sprag_host::MuteWord::Mute { said } => println!(
                    // ⚠⚠ IT STATES THE RULE, NOT THE OUTCOME. A daemon predating register item 709
                    // does not set a mute reporter aside, and one that does has a sweep-interval of
                    // lag before it has; in either window a sentence claiming *this pane is answered
                    // by its screen* would contradict the `source=` on the line ABOVE it, on one
                    // screen. Which of the two is answering is already printed there — what this
                    // sentence owes is the fact and the rule.
                    "    ⚠ THAT REPORTER IS MUTE: its last attempt failed — {said}. A report does \
                     not outrank the screen while that stands, and it clears itself the moment a \
                     delivery succeeds — the line above says which of the two is answering now.",
                ),
                // ⚠⚠ SAID, NOT SWALLOWED. Nothing prunes these (item 700's stated residue), so the
                // file will be met again — and a reader told only *no mute* would go on to find it
                // by hand and read it exactly as the watcher did.
                sprag_host::MuteWord::Inherited {
                    said,
                    left_in,
                    asking,
                } => println!(
                    "    A mute breadcrumb sits under this pane's NUMBER and is not this \
                     reporter's: it was left in generation {left_in} and this daemon is {asking}, \
                     so its subject is a pane that no longer exists. It said {said:?}. The state \
                     above stands.",
                ),
                sprag_host::MuteWord::Unattributed { said, silent } => println!(
                    "    A mute breadcrumb sits under this pane's NUMBER and cannot be attributed \
                     — {} — so it is not acted on. It said {said:?}. A pane number is reissued by \
                     the next daemon's counter, and a breadcrumb with no generation beside it could \
                     belong to any earlier holder of this number.",
                    match silent {
                        sprag_host::MuteSilence::Breadcrumb =>
                            "it names no generation, so it predates this check or was written by a \
                             reporter whose environment had lost it",
                        sprag_host::MuteSilence::Reader =>
                            "this daemon does not say which generation it is, so there is nothing \
                             to compare it against",
                    },
                ),
            }
            // The advice has to follow the evidence. Telling somebody to redefine a manifest rule
            // for a verdict a HOOK reported names a rule that never fired, and the edit would do
            // nothing at all.
            match source {
                Some(source) => {
                    println!(
                        "    `{source}` reported this, and a report outranks the screen. \
                         `sprag release-agent --pane {id}` hands the pane back to screen inference."
                    );
                    // ⚠⚠⚠ AND WHETHER THAT REPORTER CAN STILL SPEAK. A hook that has stopped being
                    // able to report leaves the LAST thing it managed to say standing for ever —
                    // and a state that outranks the screen and never expires is a run that cannot
                    // end its turn. Measured 2026-08-16: an hour of `working` against a pane whose
                    // screen had said `MILESTONE REACHED` the whole time, and the only sentence
                    // that explained it was written where nothing reads. See
                    // `sprag_host::hooks::note_mute`. That sentence is printed ABOVE this branch
                    // now, for register item 709's reason.
                    //
                    // ⚠⚠⚠⚠⚠ AND WHETHER THAT REPORTER IS THIS DAEMON'S IMAGE — register item 473,
                    // the QUIET half of the same hazard the sentence above covers loudly. A hook
                    // that can still speak perfectly may be speaking for code the daemon has never
                    // run, which is item 412: the numbers agree, the reports are accepted, and the
                    // verdict is evidence about another build. The fact reached the pane row one
                    // round ago and only a wire client could see it.
                    //
                    // ⚠ Asked of the DAEMON, unlike the mute check above — this comparison is
                    // exactly about the daemon, and the daemon is the party that can answer.
                    println!(
                        "{}",
                        reporter_build_report(
                            agent[sprag_host::wire::AGENT_BUILD_KEY].as_str(),
                            daemon_build.as_deref(),
                        ),
                    );
                }
                // ⚠⚠⚠⚠ AND THE ADVICE FOLLOWS THE EVIDENCE ONE STEP FURTHER — register item 709. A
                // scrape has TWO causes now: nothing ever reported for this pane, or something did
                // and its reporter cannot deliver. Sending the second case to edit a manifest would
                // point a person at a rule that is working perfectly, and away from the hook that is
                // not.
                None if matches!(word, sprag_host::MuteWord::Mute { .. }) => println!(
                    "    `{}` is the rule that fired, and it is answering because the reporter \
                     above cannot deliver — not because nothing reports for this pane. Fix the \
                     hook, not the manifest; the report takes the pane back on its own.",
                    rule.unwrap_or("(none)"),
                ),
                None => println!(
                    "    `{}` is the rule that fired. If this verdict is wrong, redefine or \
                     disable that id in an [[agent]] block in config.toml — the daemon picks the \
                     edit up on its own.",
                    rule.unwrap_or("(none)"),
                ),
            }
            // ⚠⚠⚠⚠⚠ AND WHAT IT IS RUNNING, WHICH IS WHY IT IS QUIET — register item 721. Without
            // this line the repair that item bought is INVISIBLE to a person: a pane that has said
            // nothing for twenty minutes and a pane that is twenty minutes into one build read
            // identically here, and the second is now the one a run deliberately does NOT hand to
            // anybody. Somebody asked to trust that has to be able to see the reason.
            //
            // ⚠ OUTSIDE the `blocked` branch below, because a tool call in flight is what a
            // WORKING pane has — that branch is about a pane which stopped to ask.
            if let Some(running) = agent[sprag_host::wire::AGENT_RUNNING_KEY].as_str() {
                println!(
                    "    it says it is running {running:?} — so a pane that looks quiet is quiet \
                     because of that, and a run's silence bound does not fire while it stands."
                );
            }
            // ⚠⚠⚠ AND WHAT IT IS ASKING, when it is asking. R367 put the question on this surface
            // and only the agent-facing mouth read it, so a person was told their agent was
            // waiting and had to go stare at the pane to learn what for — the exact re-derivation
            // of an already-parsed menu that R367 closed one mouth over. It matters more since
            // R369: `answer-pane` quotes the option back in the agent's own words, and a person
            // cannot quote what they were never shown.
            if state == sprag_host::wire::AGENT_BLOCKED_STATE {
                match agent.get(sprag_host::wire::ASKING_KEY) {
                    Some(asking) if !asking.is_null() => {
                        println!("    it is asking:{}", render_question(asking, "      "));
                        println!(
                            "    answer it with `sprag answer-pane {id} --asked TEXT --answer \
                             TEXT`, quoting the question and the option above. Do NOT send-keys \
                             the number: that skips the check that exactly one option carries \
                             what you meant."
                        );
                    }
                    // ⚠ THE ABSENCE IS A CLAIM, and saying so is the whole reason the key is
                    // published. This daemon looked at that screen and could not read a menu on
                    // it, whose remedy is a person — silence here would be indistinguishable from
                    // a build that never looks.
                    _ => println!(
                        "    it is waiting on something this daemon could not read as a menu, so \
                         no consent can name an option on it — look at the pane yourself."
                    ),
                }
                // ⚠⚠⚠⚠⚠ AND WHAT THE PEER SAID IT WANTS, which is the half the sentence above has
                // never been able to give (register item 452). That branch sends a person to go and
                // stare at a screen this daemon has already failed to parse — while the agent itself
                // stated its business, in prose, on the very hook that produced the word `blocked`.
                //
                // ⚠⚠ PRINTED BESIDE THE MENU RATHER THAN INSTEAD OF IT: the two are different facts
                // about the same wait — one is what can be ANSWERED, the other is what was ASKED —
                // and a pane can carry either, both, or neither.
                //
                // ⚠ QUOTED, never interpreted. Nothing here reads the sentence for an option: a
                // daemon that acted on prose it could not parse into a menu would be doing exactly
                // what the branch above refuses to do.
                if let Some(noticed) = agent[sprag_host::wire::AGENT_NOTICED_KEY].as_str() {
                    println!("    the agent says it wants a person for: {noticed:?}");
                }
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
    let mut pane: Option<String> = None;
    let mut kinds: Vec<String> = Vec::new();
    let usage = "events [-t SESSION] [--since N] [-f [--pane ID|NAME] [--kind KIND]…]";
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
                pane = Some(
                    it.next()
                        .ok_or_else(|| bad("events: --pane needs a pane id or NAME".to_owned()))?,
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
    let mut conn = connect_scoped(session.as_deref())?;
    // Resolved through the one resolver, so `--pane buildout` narrows to the pane called that
    // wherever it lives — the journal is the SESSION's, so a filter that could only name panes of
    // one window was narrower than the stream it filters.
    let filter = sprag_host::events::EventFilter::narrowing_wire(
        resolve_optional_pane(&mut conn, session.as_deref(), pane.as_deref(), "events")?
            .map(|site| site.id),
        &kinds,
    );
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
        let batch: Value = query_slot(
            &mut conn,
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
    // returns a second, every one empty; `sprag-latency`'s poll-pair row reproduces it, and names
    // the cause: the cursor a follower sends is the JOURNAL's and the scene runs away from it, so
    // `waitFor` never parks at all).
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
        // The DETAIL, for the three kinds that move an address — the new name, or the window a
        // pane moved into. Its key comes from the vocabulary itself
        // ([`EventKind::detail_key`](sprag_host::events::EventKind::detail_key)) rather than from a
        // second list here: this printer already scans a hand-written subject-key list, and one
        // copy of a vocabulary in a client is what R292 made the event TYPE names stop being.
        //
        // A kind this build does not know prints no detail, which is honest rather than clever: an
        // older CLI reading a newer daemon shows the type and the subject it can read.
        let detail = EventKind::from_wire(kind)
            .and_then(EventKind::detail_key)
            .and_then(|key| event[key].as_str())
            .map(|value| format!("\t{value}"))
            .unwrap_or_default();
        println!("{kind}\t{subject}{detail}");
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
///
/// # ⚠⚠⚠⚠⚠ `-c` — WHERE THE PANE STARTS, and why the CLI had no way to say it (item 417)
///
/// tmux's own spelling (`split-window [-c start-directory]`, asked of `tmux 3.4` rather than
/// remembered), and it closes a gap that had pushed a decision out of this product entirely:
/// [`sprag_host::wire::SPAWN_CWD_KEY`] has existed on the wire all along and **no CLI verb ever
/// sent it**, so `cwd` appeared in this binary only as something it PRINTS. A caller who needed a
/// pane to start anywhere but `$HOME` had to speak JSON-RPC directly — which the debt-loop skill
/// does, carrying a helper script for exactly this one field.
///
/// ⚠⚠ **Absent is `$HOME`, and that is the daemon's rule rather than this flag's** — see
/// `sprag_terminal`'s `start_dir`, which states the reasoning (*a pane is a place a person opens*)
/// and is the authority. This flag does not change the default; it makes the other answer
/// EXPRESSIBLE, which is what an agent's pane needs and a person's split does not.
///
/// # ⛔⛔⛔⛔⛔ `-w` — WHICH WINDOW THE PANE IS BORN IN, and why the default moved (item 754)
///
/// `-c` answered *where the pane starts* and left *whose screen it appears on* to the daemon's
/// default, which is the scoped session's CURRENT window — **whichever one a person is looking
/// at**. That is the right default for a person, whose current window is the one they are in, and
/// it is not an address at all for an agent: the same command lands somewhere different every time
/// it is run. Measured 2026-08-29 on this repository's own daemon — a process standing in window
/// `sprag`, current window `sce`, the pane born in `sce` — after a day in which this call put one
/// loop's panes across three windows, every one of them reported as a success.
///
/// So two things, and the second is the one that makes the defect unreachable:
///
/// * `-w WINDOW` names the window, for a caller that has one to name. Refused BESIDE a pane
///   target, because a pane already says which window it is in and two answers to one question is
///   the ambiguity this flag exists to remove.
/// * **With neither, a caller running inside a pane of the scoped session gets ITS OWN window** —
///   [`here_window`], which is [`Here`]'s own argument finished. A caller that is not in such a
///   pane narrows nothing and gets the daemon's current-window default, byte-identical to the
///   request this verb has always made.
fn split_window(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "split-window")?;
    let mut command: Option<Vec<String>> = None;
    let mut dir: Option<&'static str> = None;
    let mut before = false;
    let mut pane: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut window: Option<String> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--" => command = Some(it.by_ref().collect()),
            "-c" => {
                cwd = Some(
                    it.next()
                        .ok_or_else(|| bad("split-window: -c needs a directory".to_owned()))?,
                );
            }
            "-w" => {
                window = Some(
                    it.next()
                        .ok_or_else(|| bad("split-window: -w needs a window name".to_owned()))?,
                );
            }
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
                pane = Some(other.to_owned());
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
    // ⚠⚠ TWO ANSWERS TO ONE QUESTION IS THE AMBIGUITY `-w` EXISTS TO REMOVE — register item 754. A
    // pane is resolved session-wide and the site it answers CARRIES its window, so a `-w` beside it
    // is either redundant or a contradiction, and nothing here can tell which the caller meant.
    if let (Some(named), Some((_, Some(target)))) = (&window, &placement) {
        return Err(bad(format!(
            "split-window: -w {named} names a window, and pane {target} already says which window \
             it is in; give one or the other"
        )));
    }
    let mut action_args = match &command {
        Some(command) if command.is_empty() => {
            return Err(bad("split-window: `--` needs a command".to_owned()));
        }
        Some(command) => json!({ "cmd": command }),
        None => json!({}),
    };
    // Sent only when the caller named one, so the bare form stays byte-identical to the request it
    // has always made and the DAEMON keeps owning the default. A `cwd` the daemon cannot use is
    // refused there, before anything is built — this side does not second-guess it, because the
    // directory has to exist on the machine the PANE runs on and that is not always this one.
    if let Some(dir) = &cwd {
        action_args
            .as_object_mut()
            .expect("json! built an object")
            .insert(sprag_host::wire::SPAWN_CWD_KEY.to_owned(), json!(dir));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    // Resolved before the request so a NAME divides the pane it names, and so the window the split
    // must happen in is known: `split` resolves against the SCOPE's window at the daemon, which is
    // what its own docs say ("it exited, it is floating, or it is another window's").
    let target = match &placement {
        Some((_, Some(pane))) => Some(resolve_pane(
            &mut conn,
            session.as_deref(),
            pane,
            "split-window",
        )?),
        _ => None,
    };
    // A directional split is a DIFFERENT action from an append, not the same one with a flag: the
    // daemon divides a pane the caller names, and refuses when it cannot reach it.
    let action = match &placement {
        Some((dir, _)) => {
            let map = action_args.as_object_mut().expect("json! built an object");
            // Absent `pane` is the action's own "the active pane" default, so the bare form sends
            // no target rather than the CLI resolving one — the daemon holds the fact, and a
            // client that read it back to send it would be racing whoever moved it.
            if let Some(site) = &target {
                map.insert("pane".to_owned(), json!(site.id));
            }
            map.insert("dir".to_owned(), json!(dir));
            if before {
                map.insert("before".to_owned(), json!(true));
            }
            SPLIT_ACTION
        }
        None => SPAWN_ACTION,
    };
    let answer = invoke_action(
        &mut conn,
        match &target {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(action),
                action_args,
            ),
            // ⛔⛔⛔⛔⛔ WHERE A PANE NOBODY PLACED IS BORN — register item 754. The caller's `-w`,
            // else the window the caller is STANDING in, else nothing — and only that last case is
            // the daemon's current-window default, which is where every one of these went before.
            None => windowed_invoke(
                session.as_deref(),
                mux_action_path(action),
                action_args,
                window
                    .as_deref()
                    .or_else(|| here_window(session.as_deref())),
            ),
        },
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
        Err(error) => Err(error),
    }
}

/// `kill-pane [-t SESSION] PANE`: close the pane with id PANE — tmux `kill-pane`.
///
/// **It CASCADES** (R309): a window's last pane ends the window, whose session's last window ends
/// the session, whose being the last session ends the daemon. The printed sentence names every
/// level past the pane, which is the whole reason this verb stopped answering `null` — an operator
/// typing a PANE verb can end their session, and until R309 was told `killed pane 3` either way.
///
/// Closing the LAST live pane drains the daemon, so the reply can be cut short by its exit; that is
/// success, the same `server_gone` reading `kill-session` and `kill-window` make — and it is why the
/// `server` word races its own delivery rather than being guaranteed.
fn kill_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "kill-pane")?;
    let mut rest = rest.into_iter();
    // Absent ⇒ the active pane, which the DAEMON resolves (`CLOSE_ACTION`): a CLI that read it
    // back to send it would be racing whoever moved it between the two calls.
    let asked = rest.next();
    if let Some(other) = rest.next() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kill-pane: unexpected argument {other:?}"),
        ));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_optional_pane(&mut conn, session.as_deref(), asked.as_deref(), "kill-pane")?;
    let answer = invoke_action(
        &mut conn,
        match &site {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(CLOSE_ACTION),
                json!({ "id": site.id }),
            ),
            // Absent means the active pane, which the DAEMON resolves: reading it back to send it
            // would race whoever moved it between the two calls.
            None => scoped_invoke(session.as_deref(), mux_action_path(CLOSE_ACTION), json!({})),
        },
    );
    let named = site.as_ref().map_or_else(
        || "the active pane".to_owned(),
        |site| match &site.window {
            Some(window) => format!("pane {} (window {window})", site.id),
            None => format!("pane {}", site.id),
        },
    );
    match answer {
        Ok(answer) => {
            println!("{}", killed_sentence(&named, &answer, Ended::Pane));
            Ok(())
        }
        Err(error) if server_gone(&error) => {
            println!("killed {named} (server ended)");
            Ok(())
        }
        // The pane was RESOLVED, so "no such pane" is no longer among the causes: what is left is
        // the window declining to close it — which is the state, not the address.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "kill-pane: {} in {} could not be closed (it may already be gone)",
                named,
                scope_name(session.as_deref())
            ),
        )),
        Err(error) => Err(error),
    }
}

/// What a kill PRINTS: the thing the caller named, plus what the cascade took past it.
///
/// The three kill verbs share this, because they share one chain — a pane's kill can reach the
/// window, the session and the server, and every level above the one the user typed is news they
/// did not ask for and must be told. Before R309 all three printed a bare `killed <thing>` and the
/// daemon answered `null`, so `sprag kill-window 0` said the same words whether it had ended a
/// window or the session the caller was attached to.
///
/// `named` is the LEVEL the verb acts at, not a guess about the answer: it is what makes one
/// renderer able to say the right thing for three verbs. The extra clauses come from
/// [`Ended::beyond`], never from a table here — the rule
/// [`ResizeHow::why`](sprag_host::wire::ResizeHow::why) already set for five sentences across two
/// surfaces.
///
/// An answer with NO `ended` key can only come from a daemon older than the cascade, which
/// `client/hello` refuses by number. Rendered as the bare subject rather than guessed at, on
/// [`Ended::from_wire`]'s rule: reporting the cheapest link would tell somebody their session
/// survived a kill that ended it.
fn killed_sentence(named: &str, answer: &Value, level: Ended) -> String {
    let reached = answer
        .get(ENDED_KEY)
        .and_then(Value::as_str)
        .and_then(Ended::from_wire);
    match reached.and_then(|reached| reached.beyond(level)) {
        Some(beyond) => format!("killed {named} — {beyond}"),
        None => format!("killed {named}"),
    }
}

/// `resize-pane [-t SESSION] [PANE] -x COLS -y ROWS` or `[PANE] -L|-R|-U|-D [N]`: set a pane's
/// exact size, or move the boundary that bounds it — tmux's two `resize-pane` forms.
///
/// # Two forms, one verb, and never both
///
/// `-x -y` names a RECTANGLE and reaches the pane's PTY directly. BOTH dimensions are required,
/// because the wire action takes both and a terminal has no notion of "the other one, unchanged"
/// that this CLI could honestly supply: reading the pane's current size and sending it back would
/// race any client resizing the same pane.
///
/// `-L`/`-R`/`-U`/`-D` names a BOUNDARY and a distance in cells, defaulting to one. It moves the
/// division the arrangement puts between this pane and its neighbours on that axis, so the two
/// panes' sizes change together and the window's own size does not — which is why it is a different
/// wire action ([`RESIZE_PANE_ACTION`]) and not a flag on the first form. **The direction moves the
/// BOUNDARY, not the pane**: `-R` takes it right, and whether the named pane grows or shrinks
/// follows from which side of it the pane sits on, exactly as tmux behaves.
///
/// Giving both is refused here rather than at the daemon, because they are two different actions
/// and only this end knows the user typed one command.
///
/// No `cell_width`/`cell_height` is sent by the rectangle form: those carry a display's font metric
/// so the PTY's pixel winsize and XTWINOPS reports are truthful, and a shell has none. Omitting them
/// leaves the pane's last-known cell geometry untouched, which is the honest answer rather than a
/// zeroed guess.
fn resize_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "resize-pane")?;
    let mut pane: Option<String> = None;
    let mut cols: Option<u64> = None;
    let mut rows: Option<u64> = None;
    let mut toward: Option<(PaneDir, Option<u16>)> = None;
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
            // The ONE flag parser, `swap-pane`'s rule: a table of this verb's own would be the
            // third spelling of a four-word vocabulary, checked by nothing.
            other if sprag_host::keymap::direction_of(other).is_some() => {
                if toward.is_some() {
                    return Err(bad("resize-pane: give only one direction".to_owned()));
                }
                let dir = sprag_host::keymap::direction_of(other).unwrap_or(PaneDir::Left);
                toward = Some((dir, None));
            }
            // A bare number AFTER a direction is that direction's distance; before one it is the
            // pane. Order is what tells them apart, which is the order tmux's own form has.
            other
                if toward.is_some_and(|(_, cells)| cells.is_none())
                    && other.parse::<u16>().is_ok_and(|n| n > 0) =>
            {
                let cells = other.parse::<u16>().unwrap_or(1);
                toward = toward.map(|(dir, _)| (dir, Some(cells)));
            }
            _ if pane.is_none() => pane = Some(arg),
            other => return Err(bad(format!("resize-pane: unexpected argument {other:?}"))),
        }
    }
    if let Some((dir, cells)) = toward {
        if cols.is_some() || rows.is_some() {
            return Err(bad(
                "resize-pane sets an exact size (-x COLS -y ROWS) or moves a boundary \
                 (-L|-R|-U|-D [N]), not both"
                    .to_owned(),
            ));
        }
        return resize_toward(session.as_deref(), pane.as_deref(), dir, cells);
    }
    // No pane id ⇒ the active one, resolved by the daemon exactly as `kill-pane`'s is.
    let (Some(cols), Some(rows)) = (cols, rows) else {
        return Err(bad(
            "resize-pane needs both dimensions (-x COLS -y ROWS) or a direction (-L|-R|-U|-D [N])"
                .to_owned(),
        ));
    };
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_optional_pane(
        &mut conn,
        session.as_deref(),
        pane.as_deref(),
        "resize-pane",
    )?;
    let pane = site.as_ref().map(|site| site.id);
    invoke_action(
        &mut conn,
        match &site {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(RESIZE_ACTION),
                json!({ "id": site.id, "cols": cols, "rows": rows }),
            ),
            None => scoped_invoke(
                session.as_deref(),
                mux_action_path(RESIZE_ACTION),
                json!({ "cols": cols, "rows": rows }),
            ),
        },
    )
    .map(|_: Value| ())
    // The pane was RESOLVED, so a refusal is the kernel declining the winsize — the disjunction
    // that used to lead with "no such pane" had a half this verb can no longer produce.
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "resize-pane: {cols}x{rows} was refused for {}",
                    pane.map_or_else(
                        || "the active pane".to_owned(),
                        |pane| format!("pane {pane}")
                    ),
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

/// The BOUNDARY half of `resize-pane` — see that verb for the grammar.
///
/// Split out rather than inlined because it is a different wire action with a different answer, and
/// folding two request/answer pairs into one flat function is how a verb ends up with a refusal
/// sentence that names the wrong thing.
fn resize_toward(
    session: Option<&str>,
    pane: Option<&str>,
    dir: PaneDir,
    cells: Option<u16>,
) -> io::Result<()> {
    let mut conn = connect_scoped(session)?;
    let site = resolve_optional_pane(&mut conn, session, pane, "resize-pane")?;
    let ask = ResizeAsk {
        pane: site.as_ref().map(|site| PaneId(site.id)),
        dir,
        cells: cells.unwrap_or(ResizeAsk::CELLS_DEFAULT),
    };
    let params = match &site {
        Some(site) => site_invoke(
            session,
            site,
            mux_action_path(RESIZE_PANE_ACTION),
            ask.to_args(),
        ),
        None => scoped_invoke(session, mux_action_path(RESIZE_PANE_ACTION), ask.to_args()),
    };
    // The session was pre-flighted, so a refusal is one of the two things the daemon refuses for.
    // Which one is this end's to guess, because `InvokeError::Rejected` carries no payload
    // (upstream PINION-PR82) — and unlike the disjunctions that class usually produces, both halves
    // have a remedy a reader can act on.
    //
    // **THE VERSION SKEW IS TOLD APART, and R297's skew run is why.** This verb was the FIRST thing
    // a daemon could be too old for that is not a bump: a `WIRE_PROTOCOL` change is refused by
    // number at `client/hello`, but an ADDED ACTION leaves the handshake happy and fails at the
    // invoke. Measured against a parent-commit daemon before that arm existed, the sentence below
    // claimed *"there is no pane 0"* about a pane that was there, with the window pinned. The arm
    // is now [`invoke_action`]'s, for every verb at once — this one had it and the twenty-one
    // beside it did not.
    // No verb-specific sentence any more: `ResizeRefusal` states the fact at the end that
    // OBSERVES it, and the four causes this used to guess between are four arms there (R325).
    let answer: Value = invoke_action(&mut conn, params)?;
    println!(
        "{}",
        resize_sentence(
            ResizeHow::from_wire(
                answer[sprag_host::wire::OUTCOME_KEY]
                    .as_str()
                    .unwrap_or_default()
            )
            .unwrap_or(ResizeHow::Resized),
            ask,
            answer["pane"].as_u64().unwrap_or_default(),
            u16::try_from(answer["cells"].as_u64().unwrap_or_default()).unwrap_or(u16::MAX),
        )
    );
    Ok(())
}

/// What the BOUNDARY form of `resize-pane` prints, as a pure function of the daemon's answer — so
/// every one of the five outcomes is pinned by a unit test rather than only by whichever of them a
/// live daemon can be driven into ([`swap_sentence`]'s rule, one verb over).
///
/// `pane` is the pane AS RESOLVED, which is what makes the sentence name a pane the caller may not
/// have typed. `cells` is how far the boundary actually went, and the sentence says so when that is
/// less than was asked — the fact a caller cannot recover from an outcome word, and the one this
/// verb exists to report.
///
/// **The nothing-happened sentences come from [`ResizeHow::why`]**, not from a table here: four of
/// them would otherwise be copied into whatever surface reports next, which is the shape this
/// project keeps finding drifted.
fn resize_sentence(how: ResizeHow, ask: ResizeAsk, pane: u64, cells: u16) -> String {
    match how.why(ask.dir) {
        Some(why) => format!("pane {pane} not resized: {why}"),
        None if cells < ask.cells => format!(
            "moved pane {pane}'s {} boundary {cells} cell{} of the {} asked for; it stopped at the \
             last cell the far side may keep",
            ask.dir.wire_str(),
            if cells == 1 { "" } else { "s" },
            ask.cells,
        ),
        None => format!(
            "moved pane {pane}'s {} boundary {cells} cell{}",
            ask.dir.wire_str(),
            if cells == 1 { "" } else { "s" },
        ),
    }
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
    let mut pane: Option<String> = None;
    let mut literal = false;
    let mut tokens: Vec<String> = Vec::new();
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-l" | "--literal" => literal = true,
            // Everything after `--` is payload, so a literal `-l` or a key named `-t` can be sent.
            "--" => tokens.extend(it.by_ref()),
            _ if pane.is_none() => pane = Some(arg),
            _ => tokens.push(arg),
        }
    }
    let pane = pane.ok_or_else(|| bad("send-keys needs a pane id or NAME".to_owned()))?;
    if tokens.is_empty() {
        return Err(bad(format!(
            "send-keys needs at least one {}",
            if literal { "string" } else { "key name" }
        )));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    // This verb addresses the PANE's own external, which is reached through a WINDOW — so it is
    // resolved rather than pre-flighted, and a wrong id is still an error about panes rather than
    // an unknown address.
    let site = resolve_pane(&mut conn, session.as_deref(), &pane, "send-keys")?;
    let pane = site.id;
    for token in &tokens {
        let (path, action_args) = if literal {
            (TEXT_ACTION, json!({ "text": token }))
        } else {
            let (key, mods) = parse_key_token(token)?;
            let (ctrl, alt, shift) = mods;
            // ⚠⚠⚠ THE WIRE NAMES ITS OWN FIELDS — register item 559. This literal was the FOURTH
            // writer, and the item's own filing named only three: a scan found it. ⚠ It also sent
            // no `super`, because `parse_key_token` answers three modifiers — the constructor now
            // sends `false`, which is what the parser already read an absent key as.
            (
                KEY_ACTION,
                sprag_host::wire::keystroke_args(
                    &key,
                    sprag_input::Modifiers {
                        ctrl,
                        alt,
                        shift,
                        sup: false,
                    },
                    None,
                ),
            )
        };
        let answer = invoke_action(
            &mut conn,
            site_invoke(
                session.as_deref(),
                &site,
                pane_input_path(pane, path),
                action_args,
            ),
        )
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
        // ⚠ On stderr, and NOT folded into the count below: a person piping `send-keys` reads the
        // line that says what was sent, and a warning hidden in that line is a warning they filter
        // out. It is also per-token, because which key was swallowed is the whole content.
        for caveat in unsignalled_caveats(&answer) {
            eprintln!("send-keys: {caveat}");
        }
    }
    println!(
        "sent {} {} to pane {pane}",
        tokens.len(),
        if literal { "string(s)" } else { "key(s)" }
    );
    Ok(())
}

/// The sentences an injection's answer earns when what it wrote MEANT a signal the pane will not
/// raise — see [`sprag_host::wire::UNSIGNALLED_KEY`]. Empty when there is nothing to report.
///
/// ⚠ Read back through each vocabulary's own `from_wire`, so a word this build does not know is
/// SILENCE rather than a confidently wrong sentence.
fn unsignalled_caveats(answer: &Value) -> Vec<String> {
    let Some(entries) = answer.get(UNSIGNALLED_KEY).and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let key = entry
                .get(UNSIGNALLED_WHICH_KEY)
                .and_then(Value::as_str)
                .and_then(SignalKey::from_wire)?;
            let why = entry
                .get(UNSIGNALLED_WHY_KEY)
                .and_then(Value::as_str)
                .and_then(Unraised::from_wire)?;
            Some(format!(
                "{} was written as a byte and raised NO signal, because {}. Nothing was \
                 stopped — use `sprag {} <pane>` to send the signal itself.",
                key.chord(),
                why,
                // The verb's own name, from the vocabulary that owns it: this sentence points a
                // person at a command they will type, so a spelling invented here is a spelling
                // that can stop existing without anything noticing.
                Verb::StopJob.name(),
            ))
        })
        .collect()
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
/// `--line-breaks screen|program` says WHOSE line breaks the output carries: `screen` (the default)
/// where the terminal wrapped each line at the pane's current width, `program` where the child
/// ended it. The width is set by whoever attached a client, so anything piping this into a matcher
/// wants `program` — otherwise the same pane's output differs between two runs and neither says so.
///
/// ⚠⚠ **THE MOUTHS AGREE, and this doc used to promise that while one of them had grown a second
/// answer.** It said the text is *"the same read the `read_pane` MCP tool makes, so an agent and a
/// shell see one definition of what a pane's output IS rather than two"* — true until `read_pane`
/// learned `line_breaks` and this had not. The word, its values and the address each names are
/// [`LineBreaks`]'s, spelled once for both mouths.
fn capture_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "capture-pane")?;
    let mut pane: Option<String> = None;
    let mut breaks = LineBreaks::default();
    let mut want_breaks = false;
    for arg in rest {
        match arg.as_str() {
            _ if want_breaks => {
                want_breaks = false;
                breaks = LineBreaks::from_wire(&arg).ok_or_else(|| {
                    bad(format!(
                        "capture-pane: --line-breaks must be one of {:?}, not {arg:?}",
                        LineBreaks::ALL.map(LineBreaks::wire_str),
                    ))
                })?;
            }
            "--line-breaks" => want_breaks = true,
            // tmux's "print to stdout", which is the only thing this can do; see the doc above.
            "-p" | "--print" => {}
            _ if pane.is_none() => pane = Some(arg),
            other => return Err(bad(format!("capture-pane: unexpected argument {other:?}"))),
        }
    }
    if want_breaks {
        return Err(bad("capture-pane: --line-breaks needs a value".to_owned()));
    }
    let pane = pane.ok_or_else(|| bad("capture-pane needs a pane id or NAME".to_owned()))?;
    let mut conn = connect_scoped(session.as_deref())?;
    // Resolved rather than pre-flighted: this addresses the pane's OWN external, and that path is
    // reached THROUGH a window — so a pane one window over answered `NoExternalAtPath` until the
    // request learned to say which window. A wrong id still surfaces as "no such pane" rather than
    // as an unknown address, which is what the pre-flight was for and what the resolver keeps.
    let site = resolve_pane(&mut conn, session.as_deref(), &pane, "capture-pane")?;
    let answer: Value = query_slot(
        &mut conn,
        site_params(
            session.as_deref(),
            &site,
            pane_input_path(site.id, breaks.slot()),
        ),
    )?;
    // ⚠ The two addresses answer in two SHAPES — a string and an array of lines — because a `\n`
    // inside a joined string cannot say whether the program or the terminal put it there. Joined
    // here for stdout, and unambiguously so: the caller named which breaks they wanted.
    let joined;
    let text = match breaks {
        LineBreaks::Screen => answer.as_str().unwrap_or_default(),
        LineBreaks::Program => {
            joined = answer
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .map(|line| line.as_str().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            &joined
        }
    };
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

/// The session a `-t`-taking verb acts on: the one the caller named, or the one this process is
/// RUNNING IN — pre-flighted either way.
///
/// # `-t` is required of a caller who could not otherwise be known
///
/// These verbs demand a target because a window lives IN a session and the daemon holds several,
/// so a command from a shell somewhere outside the workspace genuinely cannot be placed. A command
/// run inside a PANE can: the daemon said which pane at that pane's birth, and [`Here`] reads it
/// back. Requiring `-t` of that caller asks it to name a session it is already standing in — and,
/// worse, invites it to name the wrong one, which is the failure the ambient scope exists to
/// remove for the verbs whose `-t` was already optional.
///
/// The refusal is unchanged for the caller who really cannot be placed, and it is the SAME sentence
/// as before: a shell that forgot `-t` learns exactly what it learnt yesterday.
fn require_target(conn: &mut HostConn, session: Option<&str>, command: &str) -> io::Result<String> {
    let session = effective_scope(session)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{command}: a target session is required (-t SESSION)"),
            )
        })?
        .to_owned();
    require_session(conn, &session)?;
    Ok(session)
}

/// `windows -t SESSION`: one line per window — its name, and `(current)` on the active one.
fn windows(args: Vec<String>) -> io::Result<()> {
    let (session, _rest) = target_and_rest(args, "windows")?;
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "windows")?;
    let windows = query_slot(
        &mut conn,
        json!({ "session": session, "path": mux_action_path(WINDOWS_SLOT) }),
    )?;
    for window in windows.as_array().into_iter().flatten() {
        let name = window["name"].as_str().unwrap_or("?");
        let marker = if window["current"].as_bool().unwrap_or(false) {
            " (current)"
        } else {
            ""
        };
        // WHO ASKED for it, when anybody did — an operator's view of the fact R313 records, and the
        // answer to "where did this window come from?" for a person who did not make it. Absent for
        // a window nobody claims, which is every window a person made: a line on all of them would
        // be noise, and the interesting case is the one that is not theirs.
        let opened_by = match window["opened_by"].as_u64() {
            Some(opener) => format!("  opened by pane {opener}"),
            None => String::new(),
        };
        println!("{name}{marker}{opened_by}");
    }
    Ok(())
}

/// `new-window -t SESSION [name]`: create + select a window, born with a shell, and print the
/// name it got (the registry allocates the lowest free one when none is given).
fn new_window(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "new-window")?;
    // `-d` is tmux's, and it is the only flag this verb has: create the window and LEAVE the
    // session where it is. Parsed positionally-blind so `new-window -d logs` and `new-window logs
    // -d` are the same command, which is what a person types.
    let mut detached = false;
    let mut name = None;
    let mut cwd: Option<String> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-d" | "--detached" => detached = true,
            // tmux's `new-window [-c start-directory]` — item 417's half, and the same flag its two
            // siblings take, so a caller does not have to remember which verb can say it.
            "-c" => {
                cwd = Some(it.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "new-window: -c needs a directory".to_owned(),
                    )
                })?);
            }
            _ if name.is_none() => name = Some(arg),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("new-window: unexpected argument {other:?}"),
                ));
            }
        }
    }
    // The grammar, parsed with the daemon's own function before the request is built — see
    // [`rename_window`] for why that is one authority with two callers rather than two authorities.
    // It is also what keeps the refusal below a single cause.
    if let Some(name) = &name {
        sprag_terminal::WindowName::parse(name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    }
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "new-window")?;
    // Through the grammar TYPE, not a key spelled here: `WindowBirthAsk` is the one place these
    // keys are written down, so the CLI, the agent surface and the daemon cannot come to spell them
    // three ways — and it is what the wire's shape pin holds the protocol number against.
    let mut action_args = Value::Object(
        WindowBirthAsk(sprag_terminal::WindowBirth {
            detached,
            opened_by: None,
        })
        .to_args(),
    );
    if let Some(name) = &name {
        action_args["name"] = json!(name);
    }
    if let Some(dir) = &cwd {
        action_args[sprag_host::wire::SPAWN_CWD_KEY] = json!(dir);
    }
    let answer = invoke_action(
        &mut conn,
        json!({ "session": session, "path": mux_action_path(NEW_WINDOW_ACTION), "args": action_args }),
    );
    match answer {
        Ok(answer) => match answer.as_str() {
            Some(created) => {
                // WHICH window the session is on afterwards is the thing `-d` changes, so the line
                // says it. A verb whose only observable difference is invisible in its own output
                // is one a script cannot check and a person cannot learn from.
                if detached {
                    println!("{created} (not selected)");
                } else {
                    println!("{created}");
                }
                Ok(())
            }
            None => Err(io::Error::other("new-window did not answer with a name")),
        },
        // The only refusal left for an explicitly-named window is a duplicate: the session was
        // pre-flighted and the name's grammar was checked above, so what surfaces as `Other` here
        // has one cause.
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
    let word = rest.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "select-window needs a window name, or -n / -p to step along the ring",
        )
    })?;
    // tmux's own flags for `next-window` / `previous-window`, on the verb sprag already has. The
    // RING is walked by the daemon: a CLI that read the window list and named the next one would be
    // a second answer to this question, computed from a list that can be a request old.
    let ask = match word.as_str() {
        "-n" => SelectWindowAsk::Step(OrderStep::Next),
        "-p" => SelectWindowAsk::Step(OrderStep::Previous),
        // A person TYPED this, so it means whatever holds it when they press Enter — the reading
        // `WindowRef::Named` exists for. The CLI has no identity to spell and should not: a window
        // id is minted at runtime and appears on no surface a person reads.
        window => SelectWindowAsk::At(sprag_host::wire::WindowRef::Named(window.to_owned())),
    };
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "select-window")?;
    let landed = scoped_window_action(
        &mut conn,
        &session,
        SELECT_WINDOW_ACTION,
        ask.to_args(),
        &format!("no window named {word:?} in session {session:?}"),
    )?;
    // The window the DAEMON says it landed on, never the argument: a step could not name one, and
    // answering the argument on the other arm would be the mistake R295 fixed for `rename-pane` and
    // R302 met again for `rename-session`.
    println!("selected {}", landed.as_str().unwrap_or(&word));
    Ok(())
}

/// `move-window [WINDOW] <--first | --last | -n | -p | --before W | --after W> -t SESSION`: move a
/// window's PLACE in its session's order — tmux `move-window`.
///
/// WINDOW is optional and defaults to the session's CURRENT window, which is the one a person at a
/// keyboard means. It is resolved by the DAEMON, never here: a CLI reading the window list to find
/// "the current one" would name it off a list that can be a request old.
///
/// # Why the anchor flags take a NAME and there is no `-t INDEX`
///
/// tmux's `move-window -t 5` names a slot by number, which works there because a tmux window HAS a
/// number. A sprag window has a NAME and that name is its address, so a number here would be a
/// second address for the same thing — and a positional one, which is exactly what
/// [`RENAME_PANE_ACTION`] exists to keep an agent from holding. `-n` / `-p` are the SAME two letters
/// `select-window` uses for the same two directions; only the WRAP differs, and this verb does not.
///
/// The four outcomes get four sentences, from the daemon's own
/// [`PlaceHow`] word — because "nothing happened" has three causes here
/// and a caller re-reading `sprag windows` cannot tell them apart.
fn move_window(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = target_and_rest(args, "move-window")?;
    let mut window: Option<String> = None;
    let mut place: Option<WindowPlace> = None;
    let mut rest = rest.into_iter();
    while let Some(arg) = rest.next() {
        // An anchored flag needs its window; the four bare ones do not. Read in one loop so a
        // second placing is caught wherever it appears rather than only after a positional.
        let anchored = |wrap: fn(String) -> WindowPlace,
                        rest: &mut std::vec::IntoIter<String>|
         -> io::Result<WindowPlace> {
            rest.next().map(wrap).ok_or_else(|| {
                bad(format!(
                    "move-window: {arg} needs the name of a window to anchor to"
                ))
            })
        };
        let named = match arg.as_str() {
            "--first" => Some(WindowPlace::First),
            "--last" => Some(WindowPlace::Last),
            "-n" => Some(WindowPlace::Step(OrderStep::Next)),
            "-p" => Some(WindowPlace::Step(OrderStep::Previous)),
            "--before" => Some(anchored(WindowPlace::Before, &mut rest)?),
            "--after" => Some(anchored(WindowPlace::After, &mut rest)?),
            other if other.starts_with('-') => {
                return Err(bad(format!("move-window: unknown flag {other:?}")));
            }
            other => {
                if window.replace(other.to_owned()).is_some() {
                    return Err(bad(format!("move-window: unexpected argument {other:?}")));
                }
                None
            }
        };
        if let Some(named) = named
            && place.replace(named).is_some()
        {
            return Err(bad(
                "move-window takes exactly one place: --first, --last, -n, -p, --before W or \
                 --after W"
                    .to_owned(),
            ));
        }
    }
    let place = place.ok_or_else(|| {
        bad(
            "move-window needs a place: --first, --last, -n, -p, --before WINDOW or --after WINDOW"
                .to_owned(),
        )
    })?;
    let ask = MoveWindowAsk {
        window: window.clone(),
        place: place.clone(),
    };
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "move-window")?;
    // The skew is told APART from a refusal — R307's finding on the verb it added, now
    // [`invoke_action`]'s job for every verb: an ADDED ACTION passes `client/hello`, so a
    // daemon that predates this verb refuses it, and reporting that as "no such window" sends the
    // user hunting for a typo in a name that is fine.
    let answer = invoke_action(
        &mut conn,
        json!({
            "session": session,
            "path": mux_action_path(MOVE_WINDOW_ACTION),
            "args": ask.to_args(),
        }),
        // The anchor-versus-subject distinction this used to make is the registry's own
        // `SessionError::UnknownAnchor` now — made at the end that resolves the two separately,
        // rather than reconstructed here from the arguments that were sent.
    )?;
    let Some((moved, how)) = MoveWindowAsk::read_answer(&answer) else {
        return Err(io::Error::other(
            "move-window: this daemon answered something this build cannot read",
        ));
    };
    // One sentence per outcome, and the three that did NOT move say which nothing it was. The
    // window is the DAEMON's resolved name, so a caller that omitted it learns which one it meant.
    println!(
        "{}",
        match how {
            PlaceHow::Moved => format!("moved {moved}"),
            PlaceHow::AlreadyThere => format!("{moved} is already there"),
            PlaceHow::Alone => format!("{moved} is this session's only window"),
            PlaceHow::Itself => format!("{moved} cannot be anchored to itself"),
        }
    );
    Ok(())
}

/// `select-pane [-t SESSION] [PANE | -L|-R|-U|-D [--from PANE]]`: make a pane active — tmux
/// `select-pane`.
///
/// A pane id and a direction name the same thing two ways, so exactly one is given. A direction
/// with no neighbour is not an error: it prints where the caller still is, because walking into the
/// edge of a layout is what a keybinding does at the edge, not a mistake it should fail on.
///
/// `--from` names the pane the step is measured FROM, for a caller that is not asking about where
/// the user happens to be — a script that knows which pane it means. There is no `--from-here`
/// beside it and that is a decision rather than an omission: a process inside a pane is already
/// handed its own id in `SPRAG_PANE`, so its shell expands `--from "$SPRAG_PANE"` with no help from
/// this binary. The MCP tool needs the flag this verb does not, because the agent calling it is not
/// the process holding the environment.
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
    let mut pane: Option<String> = None;
    let mut from: Option<String> = None;
    let mut rest = rest.into_iter();
    while let Some(arg) = rest.next() {
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
            None if arg == FROM_FLAG => {
                if from.is_some() {
                    return Err(bad(format!(
                        "select-pane: {FROM_FLAG} names one pane to step from; give only one"
                    )));
                }
                from = Some(rest.next().ok_or_else(|| {
                    bad(format!("select-pane: {FROM_FLAG} needs a pane id or NAME"))
                })?);
            }
            None => {
                if pane.is_some() {
                    return Err(bad(format!(
                        "select-pane: unexpected argument {arg:?} (one pane id, or one direction)"
                    )));
                }
                pane = Some(arg);
            }
        }
    }
    // Resolved BEFORE the arms so a name is a pane wherever it lives, and so the window the
    // request must be narrowed to is known: `select_pane` resolves against the SCOPE's window at
    // the daemon (measured), and a direction is walked within one window — the origin's.
    let mut conn = connect_scoped(session.as_deref())?;
    let target = resolve_optional_pane(
        &mut conn,
        session.as_deref(),
        pane.as_deref(),
        "select-pane",
    )?;
    // The label carries WHICH argument named the pane, because the two roles refuse differently
    // and the daemon cannot tell them apart for us (`InvokeError::Rejected` has no payload).
    let origin = resolve_optional_pane(
        &mut conn,
        session.as_deref(),
        from.as_deref(),
        "select-pane --from",
    )?;
    let site = target.clone().or_else(|| origin.clone());
    let pane = target.as_ref().map(|site| site.id);
    let from = origin.as_ref().map(|site| site.id);
    let ask = match (pane, dir) {
        (Some(pane), None) if from.is_none() => SelectAsk::Pane(PaneId(pane)),
        (Some(_), None) => {
            return Err(bad(format!(
                "select-pane: {FROM_FLAG} is where a DIRECTION starts from, and a pane id is \
                 already the target; give a direction to step, or the pane alone to select it"
            )));
        }
        (None, Some(dir)) => SelectAsk::Toward {
            dir,
            from: from.map(PaneId),
        },
        (None, None) => {
            return Err(bad(format!(
                "select-pane needs a pane id or a direction: sprag select-pane PANE | \
                 -L|-R|-U|-D [{FROM_FLAG} PANE]"
            )));
        }
        (Some(_), Some(_)) => {
            return Err(bad(format!(
                "select-pane: a pane id and a direction name the same target two ways; give one \
                 — or {FROM_FLAG} PANE to step a direction from that pane"
            )));
        }
    };
    let answer = invoke_action(
        &mut conn,
        match &site {
            Some(site) => site_invoke(
                session.as_deref(),
                site,
                mux_action_path(SELECT_PANE_ACTION),
                ask.to_args(),
            ),
            None => scoped_invoke(
                session.as_deref(),
                mux_action_path(SELECT_PANE_ACTION),
                ask.to_args(),
            ),
        },
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(
                io::ErrorKind::NotFound,
                // Which pane the daemon could not find is this end's to say, because
                // `InvokeError::Rejected` carries no payload (upstream PINION-PR82) and only the
                // caller knows whether the id it sent was a target or an origin.
                // Both panes RESOLVED, so "no such pane" is no longer among the causes: what
                // is left is the arrangement refusing the step.
                match (pane, from) {
                    (Some(pane), _) => format!("pane {pane} cannot be selected here"),
                    (None, Some(from)) => {
                        format!("pane {from} is not in an arrangement to step from")
                    }
                    (None, None) => "this session's current window holds no panes".to_owned(),
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
        select_sentence(SelectHow::read(&answer, ask.toward()), ask, selected)
    );
    Ok(())
}

/// The `select-pane` flag naming the pane a direction steps FROM — spelled once, because both the
/// parse and four refusal sentences name it.
const FROM_FLAG: &str = "--from";

/// What `select-pane` prints, as a pure function of the daemon's answer — so every one of the four
/// outcomes is pinned by a unit test rather than only by whichever of them a live daemon can be
/// driven into.
///
/// `ask` is the request this answer replies to. It is an argument rather than a field of the outcome
/// because the outcome is the DAEMON's fact and the phrasing is this surface's: only the caller knows
/// it said `-L`, and only that makes "nothing to the left of 0" sayable.
///
/// **The two nothing-happened sentences name the ORIGIN, never the landed pane**, and those are the
/// same pane only until a request names one: `-L --from 7` at pane 7's left edge leaves the user
/// where they were, and the pane with nothing to its left is 7. Printing `pane` there would report
/// an edge of a pane the caller never asked about.
///
/// **No arm can panic**, deliberately. An outcome word can only reach here from a daemon, so an
/// `at_edge` answered to a request that named a PANE is a wrong answer that parses — and this
/// degrades to the true half of it ("nothing moved") instead of turning a rendering into a crash,
/// which is the failure mode `sprag list-keys`' own flag table had until R299.
fn select_sentence(how: SelectHow, ask: SelectAsk, pane: u64) -> String {
    // The pane a "nothing that way" sentence is ABOUT: the origin the caller named, or — for a
    // request that named none — the active pane it stepped from, which is where it still is.
    let origin = ask.origin().map_or(pane, |from| from.0);
    match (how, ask.toward()) {
        (SelectHow::Moved, _) => format!("selected {pane}"),
        // Named for what the caller ASKED and could not have: an edge press is not "already on".
        (SelectHow::AtEdge, Some(dir)) => format!("nothing {} {origin}", dir.beyond()),
        // No remedy is offered for the float itself, and that is not an omission: NO CLI verb docks
        // a pane (`SET_FLOATING_ACTION` appears nowhere in this binary), so "dock it" would name an
        // action this surface cannot perform. What it can do is take a pane id, and it says so.
        (SelectHow::Untiled, _) => format!(
            "{origin} is floating, so nothing is beside it in any direction; name a pane to move to"
        ),
        (SelectHow::AlreadyActive | SelectHow::AtEdge, _) => format!("already on {pane}"),
    }
}

/// `rename-window -t SESSION [window] NEW`: rename a window (default: the current one) to NEW.
///
/// The name is parsed HERE as well as by the daemon, and that is not a second authority: it is
/// [`WindowName`](sprag_terminal::WindowName) — the daemon's own function — called by a second
/// caller, the shape `direction_of` already has. It buys the one thing the wire cannot deliver
/// while PINION-PR82 is unlanded: a `Rejected` carries no payload, so checking the grammar before
/// sending is what turns a three-way disjunction into a refusal that names the rule the user broke.
/// The daemon still parses; nothing here decides.
fn rename_window(args: Vec<String>) -> io::Result<()> {
    let (session, mut rest) = target_and_rest(args, "rename-window")?;
    let new = rest.pop().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename-window needs a new name",
        )
    })?;
    sprag_terminal::WindowName::parse(&new)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    // An optional leading positional names the window to rename; absent ⇒ the current one.
    let window = rest.pop();
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "rename-window")?;
    let action_args = match &window {
        Some(window) => json!({ "window": window, "name": new }),
        None => json!({ "name": new }),
    };
    let answer = scoped_window_action(
        &mut conn,
        &session,
        RENAME_WINDOW_ACTION,
        action_args,
        // TWO causes, not three: the grammar was checked above, so what is left is a window that
        // is not there and a name that is already another window's.
        &format!("rename-window: window not found, or {new:?} is already taken"),
    )?;
    // What the DAEMON recorded, never the argument — `rename-pane` prints the same way for the same
    // reason (a name is trimmed on the way in, so echoing the argument would report a name the
    // window does not have).
    //
    // An ABSENT name is not "an older daemon" and is not degraded to the argument: a daemon that
    // answers this action at all has passed the `WIRE_PROTOCOL` handshake, which moved to 9 in the
    // same change that added the answer — MEASURED in R306's skew run, where a pre-R306 daemon is
    // refused at `client/hello` before a rename is ever sent. So there is no old-daemon case left
    // for a fallback to serve, and a fallback that printed the argument would hide a broken
    // contract behind a plausible sentence.
    match answer.get("name").and_then(Value::as_str) {
        Some(recorded) => println!("renamed to {recorded}"),
        None => {
            return Err(io::Error::other(
                "rename-window: the daemon did not say what name it recorded",
            ));
        }
    }
    Ok(())
}

/// `rename-session [-t SESSION] NEW`: rename a session (default: the daemon's default one) to NEW.
///
/// # It moves an ADDRESS, and that is what makes it different from the two renames beside it
///
/// A session name is what every `-t` takes, what every scoped connection carries and what every
/// attached client holds — so this verb is the one rename whose effect reaches outside the thing
/// renamed. The daemon carries the session's change channel and its clients' attachments across
/// with it (see [`RENAME_SESSION_ACTION`]); what it cannot
/// carry is a name already typed into somebody's shell history.
///
/// The scope is OPTIONAL, like every other non-placement verb here: `sprag rename-session prod`
/// renames the session an unscoped request lands in, which is the one a lone user has.
fn rename_session(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "rename-session")?;
    let mut rest = rest.into_iter();
    let new = rest.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename-session needs a new name",
        )
    })?;
    if let Some(other) = rest.next() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("rename-session: unexpected argument {other:?}"),
        ));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    // The causes, listed because the daemon knows which and cannot say — `InvokeError::Rejected`
    // carries no payload (upstream PINION-PR82). An UNKNOWN scope never reaches here: it is refused
    // at the door as a scope error carrying its own sentence. The version-skew case is told APART
    // rather than folded in — saying "that name is taken" to somebody whose daemon simply predates
    // the verb sends them to fix the wrong thing — and that arm is [`invoke_action`]'s now.
    let answer = invoke_action(
        &mut conn,
        scoped_invoke(
            session.as_deref(),
            mux_action_path(RENAME_SESSION_ACTION),
            json!({ "name": new }),
        ),
        // The four rules this used to list are `SessionNameError`'s and `SessionError`'s, and the
        // daemon answers with the ONE the name broke.
    )?;
    // What the DAEMON recorded, never the argument that was sent: a name is trimmed on the way in,
    // so `rename-session "  work  "` lands as `work`, and echoing the argument would report an
    // address that does not resolve. `rename-pane` prints the same way for the same reason.
    match answer.as_str() {
        Some(recorded) => println!("renamed to {recorded}"),
        None => {
            // An OLDER daemon that has this verb but answers `null` — absent-not-wrong, so the
            // argument is the best this client can say and it says it rather than nothing.
            println!("renamed to {new}");
        }
    }
    Ok(())
}

/// `kill-window -t SESSION [window]`: kill a window (default: the current one). The session's LAST
/// window ends the SESSION — and the last session ends the daemon, so the reply can be cut short by
/// the exit, which is success (the same `server_gone` handling `kill-session` uses).
fn kill_window(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "kill-window")?;
    let window = rest.into_iter().next();
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "kill-window")?;
    // The key is the GRAMMAR's, never spelled here: `WindowRef` is the one place this product
    // writes `window` vs `window_id`. This verb is the NAME caller by design — a person typed the
    // name and means whatever holds it when they press Enter — and an ABSENT window is the scoped
    // one, which is the request with no reference at all.
    let action_args = match &window {
        Some(window) => {
            let mut map = serde_json::Map::new();
            sprag_host::wire::WindowRef::Named(window.clone()).write(&mut map);
            Value::Object(map)
        }
        None => json!({}),
    };
    let answer = invoke_action(
        &mut conn,
        json!({ "session": session, "path": mux_action_path(KILL_WINDOW_ACTION), "args": action_args }),
    );
    let target = window.as_deref().unwrap_or("the current window");
    match answer {
        Ok(answer) => {
            println!("{}", killed_sentence(target, &answer, Ended::Window));
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
    // The POLICY itself and not its name: the wire spelling is `WindowSize`'s own
    // (`ResizeWindowAsk::to_args`), so a string here would be a second place that has to agree with
    // it.
    let mut from: Option<sprag_host::WindowSize> = None;
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
            "-a" => from = Some(sprag_host::WindowSize::Smallest),
            "-A" => from = Some(sprag_host::WindowSize::Largest),
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
    let delta = |less: usize, more: usize| -> Option<i32> {
        match (edges[less], edges[more]) {
            (None, None) => None,
            // A `u64` count reaches the wire as an `i32` delta; a count past that range is a typo,
            // not a resize, and the clamp at the far end is the resolver's business, not this cast's.
            // Saturating, so the difference of two clamped counts stays in the type the wire takes.
            (less, more) => Some(
                i32::try_from(more.unwrap_or(0))
                    .unwrap_or(i32::MAX)
                    .saturating_sub(i32::try_from(less.unwrap_or(0)).unwrap_or(i32::MAX)),
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

    // The flags become the ASK — one grammar, so the keys this sends are the keys the daemon reads
    // and the wire's shape pin can see them. Built from the mutually exclusive slots checked above,
    // in the same order they were counted, so a fifth spelling would have to change the count and
    // this together.
    let size = if unpin {
        SizeRequest::Clear
    } else if let Some(policy) = from {
        SizeRequest::Clients(policy)
    } else if adjust_cols.is_some() || adjust_rows.is_some() {
        // The cast the wire's own key admits; the range check is `delta`'s and the clamp at the far
        // end is the resolver's, neither of them this call's.
        SizeRequest::Adjust {
            cols: adjust_cols.unwrap_or(0),
            rows: adjust_rows.unwrap_or(0),
        }
    } else {
        // Both dimensions, guaranteed by the half-a-rectangle check above.
        SizeRequest::Exact(ClientSize {
            cols: dimension(cols, "-x")?,
            rows: dimension(rows, "-y")?,
        })
    };

    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "resize-window")?;
    let target = window.as_deref().unwrap_or("the current window");
    let answer = scoped_window_action(
        &mut conn,
        &session,
        RESIZE_WINDOW_ACTION,
        ResizeWindowAsk {
            window: window.clone(),
            size,
        }
        .to_args(),
        // Two causes in one message, because a daemon too old to state its own reason does not
        // distinguish them — the same honesty `resize_pane` already practises. A daemon of this
        // build SAYS which (`sprag_host::window::NoBasis` has an arm per cause) and R325's funnel
        // prefers that sentence over this one.
        &format!(
            "resize-window: could not resize {target} of session {session:?} — that size could not \
             be worked out (-a/-A need an attached client that has reported an area; -L/-R/-U/-D \
             need a window that already has one), or no window is named {target:?}"
        ),
    )?;
    // What the DAEMON pinned, not what was asked for: the two differ for every spelling but -x/-y,
    // and printing the request would be this CLI quietly claiming to have done the arithmetic.
    let pinned = WindowPin::read(&answer);
    match pinned.size {
        Some(size) => println!("pinned {target} to {}x{}", size.cols, size.rows),
        None => println!("un-pinned {target}"),
    }
    // The gap between storing a size and USING one, named the moment it exists rather than left for
    // the user to discover as "I resized and nothing moved" — and named by the DAEMON, which is the
    // process that arbitrates. Reading the user's file here (which is what this did until R331) put
    // the note's authority in the wrong process: a `sprag` run whose `XDG_CONFIG_HOME` differs from
    // the daemon's was wrong in both directions, silent when the pin was inert and noisy when it was
    // in force.
    if let Some(note) = pinned.note() {
        eprintln!("sprag: note: {note}");
    }
    Ok(())
}

/// A dimension the flag parser has already checked, as the `u16` the wire's rectangle takes.
///
/// The `expect`-free spelling of a value that cannot be absent here: the half-a-rectangle check
/// above is what guarantees both are present, and a cell count past `u16` is a typo rather than a
/// window. Refused with the flag the user typed, which is the only thing they can act on.
fn dimension(value: Option<u64>, flag: &str) -> io::Result<u16> {
    value
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("resize-window: {flag} is not a cell count this window could have"),
            )
        })
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
    invoke_action(
        conn,
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
    let asked = required_pane(rest.next(), "rename-pane")?;
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
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_pane(&mut conn, session.as_deref(), &asked, "rename-pane")?;
    let pane = site.id;
    let action_args = if clearing {
        json!({ "pane": pane })
    } else {
        json!({ "pane": pane, "name": new })
    };
    let answer: Value = invoke_action(
        &mut conn,
        site_invoke(
            session.as_deref(),
            &site,
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

/// `stop-job PANE [--signal WORD]` — end what a pane is RUNNING, and leave the pane standing.
///
/// # ⚠⚠ Why this is not `send-keys PANE C-c`
///
/// Because that is a BYTE and this is a SIGNAL. `send-keys` writes `0x03` onto the pane's
/// pseudoterminal and the line discipline decides what becomes of it — a program that took the
/// terminal raw has turned signal generation off, and then the byte is ordinary input. Measured: a
/// pane running `stty -isig; sleep 300` shows `^C` and keeps sleeping. This asks the daemon that
/// owns the pty to signal the group itself, so nothing depends on the terminal's modes, and the
/// answer names what received it.
///
/// ⚠ And not `kill-pane`, which ends the pane — its shell, its scrollback, its place in the layout.
/// This ends the pane's current JOB and leaves the rest.
fn stop_job(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "stop-job")?;
    let mut rest = rest.into_iter();
    let asked = required_pane(rest.next(), "stop-job")?;
    let mut signal: Option<String> = None;
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--signal" => {
                let word = rest.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "stop-job: --signal needs one of {}",
                            sprag_terminal::Stop::WIRE_WORDS.join(", "),
                        ),
                    )
                })?;
                // ⚠ Refused HERE and not at the daemon, and the reason is the sentence: a caller
                // who mistyped a word is owed the list of words, and the wire's `TypeMismatch` has
                // nowhere to carry one. The words come from the type, so the CLI cannot admit a
                // spelling the daemon does not.
                if sprag_terminal::Stop::from_wire(&word).is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "stop-job: {word:?} is not a signal this verb sends — say one of {}",
                            sprag_terminal::Stop::WIRE_WORDS.join(", "),
                        ),
                    ));
                }
                signal = Some(word);
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("stop-job: unexpected argument {other:?}"),
                ));
            }
        }
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let site = resolve_pane(&mut conn, session.as_deref(), &asked, "stop-job")?;
    let pane = site.id;
    let mut action_args = json!({ "pane": pane });
    if let Some(word) = &signal {
        action_args[sprag_host::wire::STOP_JOB_SIGNAL_KEY] = json!(word);
    }
    let answer: Value = invoke_action(
        &mut conn,
        site_invoke(
            session.as_deref(),
            &site,
            mux_action_path(sprag_host::wire::STOP_JOB_ACTION),
            action_args,
        ),
    )?;
    // WHAT WAS DELIVERED and WHAT RECEIVED IT, both from the daemon: the caller may have omitted
    // the signal, and a line that echoed the request would leave them to know the default.
    // ⚠ The wire word read back through the TYPE, so the line a person reads is prose
    // (`interrupted`) rather than the argument vocabulary (`interrupt`) — one mapping, in the one
    // place that owns it, rather than a second list of three words here.
    let delivered = answer[sprag_host::wire::STOP_JOB_STOP_KEY]
        .as_str()
        .and_then(sprag_terminal::Stop::from_wire)
        .map_or_else(|| "stopped".to_owned(), |stop| stop.to_string());
    let group = answer[sprag_host::wire::STOP_JOB_PGID_KEY]
        .as_u64()
        .unwrap_or_default();
    match answer[sprag_host::wire::STOP_JOB_LEADER_KEY].as_str() {
        Some(job) => println!("pane {pane}: {job:?} (process group {group}) — {delivered}"),
        // A group whose leader has gone still has members, and the stop still landed on it.
        None => println!("pane {pane}: process group {group} — {delivered}"),
    }
    Ok(())
}

/// A required positional PANE argument, as the caller SPELLED it — an id (unique across the whole
/// daemon; tmux names a pane `window.index` and sprag's global id is enough) or a NAME.
///
/// It does not parse a number, and that is the change R312 made: this used to answer
/// `pane id "buildout" must be a number`, one of SIX different sentences the CLI had for the same
/// refusal, none of which accepted the address every other surface takes. Which spelling this is
/// belongs to [`PaneAddress`], and resolving it needs a
/// connection this parse does not have — so the token travels and [`resolve_pane`] decides.
fn required_pane(arg: Option<String>, command: &str) -> io::Result<String> {
    arg.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{command} needs a pane id or NAME"),
        )
    })
}

/// `break-pane -t SESSION PANE [name]`: break the pane with id PANE out of its window into a NEW
/// window (born current), printing the new window's name. tmux `break-pane` — the pane's source
/// window is DERIVED from its (registry-unique) id, so only the pane id is named.
fn break_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "break-pane")?;
    let mut rest = rest.into_iter();
    let asked = required_pane(rest.next(), "break-pane")?;
    let name = rest.next();
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "break-pane")?;
    // Registry-wide at the daemon (a break derives the source window from the pane's id), so this
    // needs the NAME to resolve and no window narrowing.
    let pane = resolve_pane(&mut conn, Some(&session), &asked, "break-pane")?.id;
    let mut action_args = json!({ "pane": pane });
    if let Some(name) = &name {
        action_args["name"] = json!(name);
    }
    let answer = invoke_action(
        &mut conn,
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
        Err(error) => Err(error),
    }
}

/// `join-pane -t SESSION PANE WINDOW`: move the pane with id PANE into the window named WINDOW,
/// appending it there. A move that empties the pane's old window closes it. tmux `join-pane`.
fn join_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "join-pane")?;
    let mut rest = rest.into_iter();
    let asked = required_pane(rest.next(), "join-pane")?;
    let window = rest.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "join-pane needs a destination window",
        )
    })?;
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "join-pane")?;
    // Registry-wide at the daemon for [`break_pane`]'s reason: both windows are derived from the
    // pane's id, so this needs the NAME to resolve and no window narrowing.
    let pane = resolve_pane(&mut conn, Some(&session), &asked, "join-pane")?.id;
    // The keys are the GRAMMAR's, never spelled here: `JoinAsk` is the one place this product
    // writes `window` vs `window_id`, and a hand-built object at a fifth call site is how a client
    // comes to send an address it does not hold. This verb is the NAME caller by design — a person
    // typed the name and means whatever holds it when they press Enter.
    let ask = sprag_host::wire::JoinAsk {
        pane: sprag_terminal::PaneId(pane),
        window: sprag_host::wire::WindowRef::Named(window.clone()),
    };
    let answer = invoke_action(
        &mut conn,
        json!({ "session": session, "path": mux_action_path(JOIN_PANE_ACTION), "args": ask.to_args() }),
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
    let mut panes: Vec<String> = Vec::new();
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
                panes.push(other.to_owned());
            }
        }
    }
    let (asked, asked_target) = match panes.as_slice() {
        [pane, target] => (pane.clone(), target.clone()),
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
            "move-pane: pane {asked} needs an axis to land on beside pane {asked_target} — \
             -h (right) or -v (below); use join-pane to append into a window instead"
        ))
    })?;
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "move-pane")?;
    // Registry-wide at the daemon: BOTH windows are derived from the two pane ids (R284), so this
    // needs only the names to resolve.
    let pane = resolve_pane(&mut conn, Some(&session), &asked, "move-pane")?.id;
    let target = resolve_pane(&mut conn, Some(&session), &asked_target, "move-pane")?.id;
    let answer = invoke_action(
        &mut conn,
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
        Err(error) => Err(error),
    }
}

/// `swap-pane [-t SESSION] [PANE] <WITH | -L|-R|-U|-D>`: exchange two panes' positions — tmux
/// `swap-pane`.
///
/// PANE omitted means the session's ACTIVE pane. The partner is either a pane id or a direction,
/// exactly one of them; a direction at the edge of the layout prints "nothing to trade with" and
/// succeeds, which is what a key bound to this deserves.
///
/// # The ORIGIN is the leading positional, and there is deliberately no `--from`
///
/// `swap-pane 7 -L` measures the step from pane 7, which is exactly what
/// [`select_pane`]'s `--from` does one verb over. The two verbs spell it differently because their
/// POSITIONAL grammars differ, not because one of them lacks the argument: `select-pane`'s only
/// positional is already the target, so an origin there needs a flag, while this verb's first
/// positional has always been the pane being placed. Adding `--from` here would give ONE verb two
/// spellings of one concept, which is a drift surface rather than a convenience —
/// **and the debt register's claim that this verb "takes no origin" was measured and refuted before
/// this was written.**
///
/// # The scope is OPTIONAL here, unlike the other placement verbs
///
/// Every other PANE verb takes an optional `-t` (`split-window`, `kill-pane`, `resize-pane`,
/// `send-keys`, `capture-pane`, `select-pane`, `rename-pane`) and the placement quartet
/// (`break-pane` / `join-pane` / `move-pane` / `zoom-pane`) requires one. This verb moved to the
/// majority because R301 made it `select-pane`'s directional twin — same flags, same origin, same
/// daemon-side resolution — and a twin that cannot be typed the same way is not one. The other four
/// are unchanged and registered, because nothing this round makes them twins of anything.
fn swap_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "swap-pane")?;
    let mut dir: Option<PaneDir> = None;
    let mut panes: Vec<String> = Vec::new();
    for arg in rest {
        // The ONE flag parser ([`keymap::direction_of`](sprag_host::keymap::direction_of)), not a
        // table of this verb's own: until R299 both this and `select-pane` mapped a flag straight to
        // a WIRE word, skipping `PaneDir` — two copies of one vocabulary, checked by nothing, and the
        // second one survived the round that removed the first because nothing failed either way.
        match sprag_host::keymap::direction_of(&arg) {
            Some(named) => {
                if dir.is_some() {
                    return Err(bad("swap-pane: give only one direction".to_owned()));
                }
                dir = Some(named);
            }
            None => {
                let other = arg.as_str();
                if panes.len() == 2 {
                    return Err(bad(format!("swap-pane: unexpected argument {other:?}")));
                }
                panes.push(other.to_owned());
            }
        }
    }
    // The two shapes, and exactly one of them — the wire refuses "both" and "neither" as malformed,
    // so the CLI names the mistake here rather than letting it read as a daemon refusal. The ask is
    // a TYPE, so the combinations below are the only ones expressible past this point.
    let mut conn = connect_scoped(session.as_deref())?;
    // Resolved BEFORE the shapes below, so a NAME is a pane in whichever window holds it — the
    // daemon's swap is registry-wide (measured), so nothing here needs narrowing.
    let mut resolved = Vec::new();
    for asked in &panes {
        resolved.push(resolve_pane(&mut conn, session.as_deref(), asked, "swap-pane")?.id);
    }
    let ask = match (resolved.as_slice(), dir) {
        ([pane, with], None) => SwapAsk::With {
            pane: Some(PaneId(*pane)),
            with: PaneId(*with),
        },
        ([with], None) => SwapAsk::With {
            pane: None,
            with: PaneId(*with),
        },
        ([pane], Some(dir)) => SwapAsk::Toward {
            pane: Some(PaneId(*pane)),
            dir,
        },
        ([], Some(dir)) => SwapAsk::Toward { pane: None, dir },
        ([], None) => {
            return Err(bad(
                "swap-pane needs a pane to trade with or a direction: sprag swap-pane [PANE] \
                 <WITH | -L|-R|-U|-D>"
                    .to_owned(),
            ));
        }
        _ => {
            return Err(bad(
                "swap-pane takes a pane to swap with OR a direction (-L/-R/-U/-D), not both"
                    .to_owned(),
            ));
        }
    };
    let answer = invoke_action(
        &mut conn,
        scoped_invoke(
            session.as_deref(),
            mux_action_path(SWAP_PANE_ACTION),
            ask.to_args(),
        ),
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(
                io::ErrorKind::NotFound,
                // Which pane the daemon could not find is this end's to say, because
                // `InvokeError::Rejected` carries no payload (upstream PINION-PR82) and only
                // the caller knows which ids it sent. It stays a disjunction of the ids it
                // actually named rather than of every id in the session.
                match ask {
                    SwapAsk::With { pane, with } => format!(
                        "swap-pane refused: {} is not a tiled pane of this session",
                        match pane {
                            Some(pane) => format!("pane {}, or pane {with},", pane.0),
                            None => format!("pane {with}, or the active pane,", with = with.0),
                        }
                    ),
                    SwapAsk::Toward {
                        pane: Some(pane), ..
                    } => format!("swap-pane refused: this session holds no pane {}", pane.0),
                    SwapAsk::Toward { pane: None, .. } => {
                        "swap-pane refused: this session's current window holds no panes".to_owned()
                    }
                },
            )
        } else {
            error
        }
    })?;
    println!(
        "{}",
        swap_sentence(
            SwapHow::read(&answer, ask.toward()),
            ask,
            answer["a"].as_u64().unwrap_or_default(),
            answer["b"].as_u64(),
        )
    );
    Ok(())
}

/// What `swap-pane` prints, as a pure function of the daemon's answer — so every one of the four
/// outcomes is pinned by a unit test rather than only by whichever of them a live daemon can be
/// driven into ([`select_sentence`]'s rule, one verb over).
///
/// `a` is the pane the daemon placed and `b` the partner it resolved, which is what makes the
/// success sentence name a pane a `dir` caller never typed. The two nothing-happened sentences name
/// `a`, which is the ORIGIN — the pane whose edge was reached, or the floating one — never the
/// partner, because there is none.
///
/// **No arm can panic**, deliberately: an outcome word can only reach here from a daemon, so an
/// `at_edge` answered to a request that named a partner is a wrong answer that parses, and this
/// degrades to the true half of it rather than turning a rendering into a crash.
fn swap_sentence(how: SwapHow, ask: SwapAsk, a: u64, b: Option<u64>) -> String {
    match (how, ask.toward()) {
        (SwapHow::Swapped, _) => match b {
            Some(b) => format!("swapped pane {a} with {b}"),
            // A daemon that says it traded and names nobody to have traded with; the honest half is
            // that something moved.
            None => format!("swapped pane {a}"),
        },
        (SwapHow::AtEdge, Some(dir)) => {
            format!("nothing {} {a} to trade with", dir.beyond())
        }
        // No remedy is offered for the float itself, and that is not an omission: NO CLI verb docks
        // a pane (`SET_FLOATING_ACTION` appears nowhere in this binary), so "dock it" would name an
        // action this surface cannot perform. What it can do is take a pane id, and it says so.
        (SwapHow::Untiled, _) => format!(
            "{a} is floating, so nothing is beside it in any direction; name a pane to trade with"
        ),
        (SwapHow::SamePane | SwapHow::AtEdge, _) => {
            format!("pane {a} cannot trade places with itself")
        }
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
    let mut pane: Option<String> = None;
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
            other if pane.is_none() => pane = Some(other.to_owned()),
            other => return Err(bad(format!("zoom-pane: unexpected argument {other:?}"))),
        }
    }
    let mut conn = connect()?;
    let session = require_target(&mut conn, session.as_deref(), "zoom-pane")?;
    // Registry-wide at the daemon (measured: a bare `zoom_pane` reaches a pane one window over),
    // so this needs only the name to resolve.
    let mut args = json!({});
    if let Some(site) =
        resolve_optional_pane(&mut conn, Some(&session), pane.as_deref(), "zoom-pane")?
    {
        args["pane"] = json!(site.id);
    }
    if let Some(on) = on {
        args["on"] = json!(on);
    }
    let answer = invoke_action(
        &mut conn,
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
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The listing a PERSON greps tells a revived agent, a driven one, and a plain pane apart.
    ///
    /// ⚠⚠⚠⚠⚠ **THE DAEMON HAD ALREADY ANSWERED THIS AND THE ANSWER STOPPED AT THE WIRE** — register
    /// items 595 and 602. The two keys are published by the host and asserted at the host, and
    /// until this test existed NOTHING outside `wire.rs` read either of them: `sprag panes` built
    /// its row without ever looking, so the three panes below printed identically to the one person
    /// the fact was for. That is register item 418's failure exactly, one fact later.
    ///
    /// ⚠⚠ **THE THIRD ROW IS THE CONTROL AND IT CARRIES THE WHOLE CLAIM.** All three rows are the
    /// same program at the same size built by the same function; a marker that appeared on the
    /// plain one would be decoration, and one that appeared on none would leave the row as silent
    /// as it was. The pairing is asserted too — `(revived)` WITHOUT `(driven)` is the orphan, and
    /// it is the combination rather than either word that a person acts on.
    #[test]
    fn the_pane_listing_tells_a_revived_agent_from_a_driven_one_and_from_a_plain_pane() {
        let row = |extra: Value| {
            let mut pane = json!({"id": 7, "cols": 80, "rows": 24, "command": "claude"});
            for (key, value) in extra.as_object().expect("an object of extra keys") {
                pane[key] = value.clone();
            }
            pane_row(&pane)
        };

        let orphan = row(json!({PANE_REVIVED_KEY: true}));
        assert!(
            orphan.contains("(revived)") && !orphan.contains("(driven)"),
            "⛔⛔⛔ ITEM 595: the daemon re-ran this agent out of a snapshot and nothing is driving \
             it, and the row a person greps says neither. The whole harm is that this pane looks \
             exactly like the one they opened on purpose — the row was: {orphan:?}",
        );

        let working = row(json!({
            PANE_REVIVED_KEY: true,
            PANE_DRIVEN_KEY: true,
        }));
        assert!(
            working.contains("(revived)") && working.contains("(driven)"),
            "⚠⚠⚠⚠ A RESTORED SEAT A RUN PICKED UP IS NOT AN ORPHAN, and the row has to be able to \
             say both at once or the reader cannot tell it from the one above: {working:?}",
        );

        let plain = row(json!({}));
        assert!(
            !plain.contains("(revived)") && !plain.contains("(driven)"),
            "⚠⚠⚠⚠⚠ THE CONTROL: a person opened this one and nothing is driving it, so both \
             markers must be ABSENT rather than negated — a row that marked every pane would be \
             reporting that the daemon had restarted: {plain:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A DAEMON THAT CANNOT SAY WHICH BUILD IT IS, IS NOT A DAEMON THAT MATCHES** — the one
    /// sentence `sprag_rpc::BUILD_FIELD`'s whole argument for needing no protocol bump rests on.
    ///
    /// The key is added to the `client/hello` reply, and an added ANSWER key earns no
    /// `WIRE_PROTOCOL` bump because it is *absent-not-wrong* to an old reader. That licence is
    /// CONDITIONAL: it holds only while nobody reads the absence as a promise. The moment a reader
    /// prints "agree" for a daemon that said nothing, the key is making a claim old daemons cannot
    /// support, and it earns the number after all.
    ///
    /// So this gate pins the THREE cases as three, and the mutation it exists to catch is the one a
    /// reviewer would wave through: folding `None` into the equal arm. That reads as tidy, passes
    /// every other test in this file, and quietly converts *"nobody knows"* into *"nothing is
    /// wrong"* — which is the reading register item 438 cost a round to.
    #[test]
    fn a_daemon_that_cannot_say_which_build_it_is_is_not_a_daemon_that_matches() {
        let mine = sprag_host::wire::BUILD;

        let agreed = build_report(Some(mine));
        assert!(
            agreed.contains(mine) && agreed.contains("agree"),
            "the ordinary case names the build once and says the ends agree: {agreed:?}",
        );

        // ── A SKEW IS A FINDING, and it must carry BOTH builds and the remedy ──
        let skewed = build_report(Some("0000deadbeef"));
        assert!(
            skewed.contains(mine) && skewed.contains("0000deadbeef"),
            "⚠⚠⚠ a skew that names only one build tells a reader nothing about which is which, \
             which is the state this whole field exists to end: {skewed:?}",
        );
        assert!(
            skewed.contains("restart"),
            "⚠⚠ a finding a reader cannot act on is a footnote. The remedy is a restart and the \
             report must say so: {skewed:?}",
        );
        assert!(
            !skewed.contains("agree"),
            "two different builds do not agree: {skewed:?}",
        );

        // ── AND THE CASE NO LIVE DAEMON ON THIS MACHINE CAN PRODUCE ANY MORE ──
        // ⚠ It is reachable only from a daemon predating the key, which is exactly why it is
        // driven here as a value rather than left to a fixture that would have to be an old build.
        let silent = build_report(None);
        assert!(
            !silent.contains("agree"),
            "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE. Saying they agree here would be this \
             client inventing the answer an old daemon could not give — and it is the reading that \
             breaks the no-bump argument on `BUILD_FIELD`: {silent:?}",
        );
        assert!(
            silent.contains(mine),
            "the client still knows its own half, and a reader needs it to compare by hand: \
             {silent:?}",
        );
    }

    /// ⚠⚠⚠⚠ **FOUR ANSWERS ABOUT A REPORTER, AND NO TWO OF THEM READ ALIKE** — register item 473's
    /// renderings, pinned where the live gate cannot reach.
    ///
    /// `wire_client::a_person_is_told_whether_the_reporter_that_answered_is_this_daemons_image`
    /// drives three of these through real processes — a real hook, a real daemon, and a daemon whose
    /// stated build was made to differ. The fourth cannot be driven that way and is the one this
    /// exists for: a daemon so old it does not answer `sprag_rpc::BUILD_FIELD` at all. That is not a
    /// match and not a skew — it is nobody being able to compare, and rendering it as either would
    /// be this client inventing an answer that daemon could not give.
    ///
    /// ⚠⚠⚠ The mutation this catches is the tidy one: collapsing an arm because the sentence "reads
    /// about the same". Four distinct answers rendered as three is the defect the whole key exists
    /// to end, and it leaves every other gate in this tree green.
    #[test]
    fn four_answers_about_a_reporter_and_no_two_of_them_read_alike() {
        let mine = sprag_host::wire::BUILD;

        let same = reporter_build_report(Some(mine), Some(mine));
        assert!(
            same.contains(mine) && same.contains("own image"),
            "the ordinary case says the reporter is the code this daemon runs: {same:?}",
        );

        // ── A SKEW IS A FINDING: both builds, and something a reader can DO ──
        let skew = reporter_build_report(Some("0000deadbeef"), Some(mine));
        assert!(
            skew.contains("0000deadbeef") && skew.contains(mine),
            "⚠⚠⚠ a skew that names one build tells a reader nothing about which is which: {skew:?}",
        );
        assert!(
            skew.contains("Restart"),
            "⚠⚠ a finding a reader cannot act on is a footnote — the remedy is a restart and the \
             sentence must say so: {skew:?}",
        );

        // ── THE REPORTER SAID NOTHING, which is never agreement ──
        let unsaid = reporter_build_report(None, Some(mine));
        assert!(
            !unsaid.contains("own image"),
            "⚠⚠⚠⚠⚠ ABSENT MEANS *IT DID NOT SAY*. Every reporter older than `AGENT_BUILD_KEY` \
             answers exactly this, and reading it as a match is the inversion the key exists to \
             end: {unsaid:?}",
        );

        // ── AND THE DAEMON THAT CANNOT SAY ITS OWN — no comparison is possible at all ──
        let neither = reporter_build_report(Some(mine), None);
        assert!(
            !neither.contains("own image") && neither.contains(mine),
            "⚠⚠⚠⚠ a daemon predating the hello's build key cannot be compared against, and \
             claiming a match here would break the no-bump argument `BUILD_FIELD` rests on: \
             {neither:?}",
        );

        let all = [&same, &skew, &unsaid, &neither];
        for (i, one) in all.iter().enumerate() {
            for other in &all[i + 1..] {
                assert_ne!(
                    one, other,
                    "⚠⚠⚠⚠⚠ FOUR ANSWERS STAY FOUR. Two that render identically are one answer \
                     wearing two names, and the reader cannot tell which they were given",
                );
            }
        }
    }

    /// ⚠⚠⚠⚠⚠ **THE WINDOW `attach` LAUNCHES IS THE ONE BESIDE THIS BINARY** — register item 463's
    /// construction half, and the branch the whole claim rests on had never been run.
    ///
    /// # What was untested, and why it stayed that way
    ///
    /// *"A person cannot start a GUI that is not the daemon's build"* is true only because of the
    /// SIBLING step: `target/debug/sprag` launches the `target/debug/sprag-gui` beside it rather
    /// than whatever `PATH` finds. Every CLI gate over `attach` points `SPRAG_GUI_BIN` at a stand-in
    /// — which is right, because a test must not open a window — and that override returns at the
    /// FIRST branch, so the middle one was reached by nothing at all. It could not be: `client_bin`
    /// read `current_exe`, which is process-global, so the rule had no seam a test could drive.
    /// [`client_beside`] is that seam, split exactly as `sprag_beside` and `mcp_beside` are.
    ///
    /// ⚠⚠⚠ **The order is the substance, not a detail.** An override that lost to a sibling would
    /// make this suite's stand-ins unreachable; a sibling that lost to `PATH` would launch an
    /// installed client of unknown age against this daemon, which is the whole hazard item 463 was
    /// filed for. Each of the three is pinned against a directory this test builds, so a passing
    /// answer names a file that is really there.
    #[test]
    fn the_window_attach_launches_is_the_one_beside_this_binary() {
        let dir = std::env::temp_dir().join(format!("sprag-beside-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("a tree of this test's own");
        let exe = bin.join("sprag");
        std::fs::write(&exe, b"#!/bin/sh\n").expect("a stand-in for the running binary");
        let sibling = bin.join("sprag-gui");

        // ── NO SIBLING: the name alone, which `PATH` resolves to whatever is installed ──
        assert_eq!(
            client_beside(None, Some(&exe), "sprag-gui"),
            PathBuf::from("sprag-gui"),
            "a deployment that copied only some of the binaries still gets a window — and the \
             doubt about which build it is belongs to `doctor`, not to a refusal here",
        );

        // ── THE SIBLING EXISTS: it wins, and this is the branch nothing used to reach ──
        std::fs::write(&sibling, b"#!/bin/sh\n").expect("a sibling client beside it");
        assert_eq!(
            client_beside(None, Some(&exe), "sprag-gui"),
            sibling,
            "⚠⚠⚠⚠⚠ THIS IS THE CONSTRUCTION HALF OF ITEM 463: the window a person gets is the one \
             built beside the binary they typed, never one `PATH` chose for them",
        );

        // ── THE OVERRIDE STILL WINS, or every stand-in in this suite stops standing in ──
        assert_eq!(
            client_beside(
                Some(OsString::from("/elsewhere/gui")),
                Some(&exe),
                "sprag-gui"
            ),
            PathBuf::from("/elsewhere/gui"),
            "⚠⚠ an override that lost to the sibling would make the CLI suite's stand-ins \
             unreachable — and it is the door a person points at their own build",
        );

        // ── AND A PROCESS THAT CANNOT SAY WHERE IT IS falls through rather than guessing ──
        assert_eq!(
            client_beside(None, None, "sprag-gui"),
            PathBuf::from("sprag-gui"),
            "no exe path is no sibling to find; naming one anyway would be inventing a file",
        );
        std::fs::remove_dir_all(&dir).expect("this test leaves nothing behind");
    }

    /// ⚠⚠⚠⚠⚠ **A WINDOW THAT IS NOT THE DAEMON'S BUILD IS NAMED, AND A SET THAT MATCHES SAYS SO
    /// OUT LOUD** — register item 463's report half, pinned where a live daemon cannot reach.
    ///
    /// # What this is for
    ///
    /// The daemon RESOLVES its hook and its MCP server as siblings of the running executable, so
    /// neither can be another build without a deployment that split them. **The GUI is outside that
    /// structure**: it is launched by hand from wherever somebody points, and this repository's own
    /// promotion copies the daemon to one directory and runs `target/debug/sprag-gui`. The skew is
    /// therefore ordinary here, and until this nothing anywhere could state it.
    ///
    /// # ⚠⚠⚠⚠ The mutations this exists to catch, and both look like tidying
    ///
    /// * **Folding the silent client into the matching ones.** Every client older than
    ///   `sprag_rpc::CLIENT_BUILD_PARAM` says nothing, so that fold makes the commonest case read
    ///   as the safe one — the inversion all three build keys exist to end.
    /// * **Printing findings only.** A surface that says nothing when every window matches cannot
    ///   be told from one that did not look, and this is read exactly when somebody already
    ///   suspects the answer. The counts are the earned silence.
    ///
    /// ⚠ The daemon-silent arm is driven as a VALUE for [`build_report`]'s reason: it is reachable
    /// only from a daemon predating `sprag_rpc::BUILD_FIELD`, which is not a process this tree can
    /// build.
    #[test]
    fn a_window_that_is_not_the_daemons_build_is_named_and_a_matching_set_says_so() {
        let mine = sprag_host::wire::BUILD;
        let named =
            |client: &str, build: Option<&str>| vec![(client.to_owned(), build.map(str::to_owned))];

        // ── THE ORDINARY CASE, and it must still SAY that it looked ──
        let agreed = attached_build_report(Some(mine), &named("gui-1", Some(mine)));
        assert!(
            agreed.contains("1 attached client(s)") && agreed.contains("1 on the daemon's build"),
            "⚠⚠⚠⚠ SILENCE HAS TO BE EARNED. A reader cannot tell *checked and matched* from \
             *nobody looked* unless the count is printed: {agreed:?}",
        );
        assert!(
            !agreed.contains("NOT THIS DAEMON'S IMAGE"),
            "a window on the daemon's own build is not a finding: {agreed:?}",
        );

        // ── THE WHOLE HAZARD: the window is drawing from code this daemon has never run ──
        let skewed = attached_build_report(Some(mine), &named("gui-2", Some("0000deadbeef")));
        assert!(
            skewed.contains("NOT THIS DAEMON'S IMAGE") && skewed.contains("gui-2"),
            "⚠⚠⚠⚠⚠ the finding must NAME the window, because a person with three of them open \
             cannot act on a count: {skewed:?}",
        );
        assert!(
            skewed.contains(mine) && skewed.contains("0000deadbeef"),
            "⚠⚠⚠ and it names BOTH builds — one alone tells a reader nothing about which is \
             which: {skewed:?}",
        );

        // ── THE ARM A TIDY EDIT FOLDS INTO THE FIRST ──
        let quiet = attached_build_report(Some(mine), &named("gui-3", None));
        assert!(
            quiet.contains("1 did not say") && quiet.contains("gui-3"),
            "⚠⚠⚠⚠⚠ AN ABSENT BUILD IS NOT A MATCHING ONE. Every client older than this key sends \
             exactly this silence, and counting it as agreement would make the commonest case look \
             like the safe one: {quiet:?}",
        );
        assert!(
            !quiet.contains("1 on the daemon's build"),
            "a client that did not say was not compared, so it cannot be counted as matching: \
             {quiet:?}",
        );

        // ── THE DAEMON THAT CANNOT SAY ITS OWN: nobody can compare, and it is said ONCE ──
        let blind = attached_build_report(None, &named("gui-4", Some(mine)));
        assert!(
            blind.contains("could not be compared"),
            "⚠⚠⚠⚠ a daemon predating the hello's build key makes every row unjudgeable, and \
             claiming a match here would break the no-bump argument `BUILD_FIELD` rests on: \
             {blind:?}",
        );
        assert!(
            !blind.contains("1 on the daemon's build"),
            "there is no daemon build to be on: {blind:?}",
        );

        // ── AND A SET SAYS ONE THING PER CLIENT, not one thing per set ──
        let mixed = attached_build_report(
            Some(mine),
            &[
                ("gui-ok".to_owned(), Some(mine.to_owned())),
                ("gui-old".to_owned(), Some("0000deadbeef".to_owned())),
                ("tui-quiet".to_owned(), None),
            ],
        );
        assert!(
            mixed.contains("3 attached client(s)")
                && mixed.contains("1 on the daemon's build")
                && mixed.contains("1 on other code")
                && mixed.contains("1 did not say"),
            "⚠⚠⚠ three windows in three states are three answers, and a summary that collapses \
             them is a report about none of them: {mixed:?}",
        );
        assert!(
            mixed.contains("gui-old") && mixed.contains("tui-quiet") && !mixed.contains("gui-ok"),
            "⚠⚠ the two a reader must act on are named and the one that is fine is not — a \
             finding per healthy row is how a report stops being read: {mixed:?}",
        );

        // ── NOBODY ATTACHED IS ITS OWN ANSWER, never an empty pass ──
        let none = attached_build_report(Some(mine), &[]);
        assert!(
            none.contains("no client is attached"),
            "⚠⚠⚠ zero windows compared must not render as zero problems found: {none:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A WINDOW THAT FOLLOWS NOBODY IS NAMED, AND ONE THAT FOLLOWS ITS CLIENTS SAYS SO** —
    /// register item 482, the gate on the sentence whose ABSENCE made a working terminal read as a
    /// broken one.
    ///
    /// # What it cost to have no such sentence, measured
    ///
    /// `resize-window` pins a window and flips `window-size` to `manual`, which does not read the
    /// attached clients at all — and the value is written to the config file, so the act outlives
    /// the window, the daemon and every later session. Measured 2026-08-20: a pinned window's client
    /// reported `203x25` while its panes sat at `41x40` and `78x40`, following nobody exactly as
    /// that policy promises. **The owner read it as the product being broken and asked whether the
    /// code had been hardcoded** — a reasonable reading, because nothing anywhere said otherwise.
    ///
    /// # ⚠⚠⚠ The two mutations this exists to catch, and both look like tidying
    ///
    /// * **Printing only the findings.** A surface silent when nothing is pinned cannot be told from
    ///   one that did not look — and this is read exactly when somebody already suspects the answer.
    ///   `attached_build_report`'s earned-silence rule, one fact over.
    /// * **Naming the state without the remedy.** *"pinned"* sends a reader to search for the verb;
    ///   the un-pin is one flag and it belongs in the sentence that reports the state.
    #[test]
    fn a_pinned_window_is_named_with_its_remedy_and_an_unpinned_set_says_it_looked() {
        // ── THE ORDINARY CASE, and it must still SAY that it looked ──
        let following =
            pinned_window_report(&[("work".to_owned(), 2, 0), ("loop".to_owned(), 1, 0)]);
        assert!(
            following.contains("no window is pinned"),
            "⚠⚠⚠ ZERO PINS MUST NOT RENDER AS ZERO OUTPUT. A reader consults this because a \
             terminal is behaving oddly; silence here reads as *nobody checked* and sends them back \
             to guessing, which is the state this whole item is about: {following:?}",
        );

        // ── THE ONE A READER MUST ACT ON ──
        let pinned = pinned_window_report(&[("work".to_owned(), 3, 1)]);
        assert!(
            pinned.contains("FOLLOW NO CLIENT") && pinned.contains("\"work\""),
            "⚠⚠⚠⚠⚠ THE SESSION IS NAMED AND THE STATE IS SPELLED OUT. Until this key existed the \
             window simply stopped following and no surface could say why: {pinned:?}",
        );
        assert!(
            pinned.contains("resize-window -u -t work"),
            "⚠⚠⚠⚠ AND THE REMEDY TRAVELS WITH IT, naming the session so it can be typed rather than \
             assembled. A report that states a condition and leaves the reader to find the verb is \
             a diagnostic, not a report — this workspace's own rule for `Refusal::describe`: \
             {pinned:?}",
        );
        assert!(
            pinned.contains('1') && pinned.contains('3'),
            "⚠⚠ and it says how MANY of how many — a session can hold several windows and only one \
             may be pinned, which is why the field is a count: {pinned:?}",
        );

        // ── A MIXED MACHINE ANSWERS PER SESSION, not per machine ──
        let mixed = pinned_window_report(&[("quiet".to_owned(), 1, 0), ("stuck".to_owned(), 2, 2)]);
        assert!(
            mixed.contains("\"stuck\"") && !mixed.contains("\"quiet\""),
            "⚠⚠⚠ the session a reader must act on is named and the healthy one is not — a finding \
             per healthy row is how a report stops being read: {mixed:?}",
        );
        assert!(
            !mixed.contains("no window is pinned"),
            "⚠⚠ and the all-clear must not be printed beside a finding that contradicts it: \
             {mixed:?}",
        );
    }

    /// ⚠⚠⚠⚠ **A RUN WITH NO RECORDED BUILD SAYS SO, WHERE ONE THAT MATCHES SAYS NOTHING** — the
    /// same rule as the gate above, met at the one place that is allowed to bend it.
    ///
    /// Every other reader of this fact is forbidden to fill in an absence. [`render_build`] is the
    /// exception and the exception is narrow: it does not print the absence, it prints a COMPARISON
    /// against a value it knows for certain. So an empty clause is a positive claim — *this run was
    /// driven by the build you are running* — and the two cases it cannot resolve must both speak.
    ///
    /// ⚠⚠⚠ The mutation this catches: rendering `None` as the empty string "because it is missing
    /// anyway". Every run in `sprag runs` would then read as driven by the reader's own build,
    /// including runs a dead daemon drove — the wrong answer that decodes cleanly.
    #[test]
    fn a_run_with_no_recorded_build_says_so_where_one_that_matches_says_nothing() {
        use serde_json::json;
        let key = sprag_host::plugins::RUN_BUILD_KEY;

        assert_eq!(
            render_build(&json!({ key: sprag_host::wire::BUILD })),
            "",
            "the common case is silent, so a hundred rows are not a hundred repetitions of one \
             fact",
        );

        let other = render_build(&json!({ key: "0000deadbeef" }));
        assert!(
            other.contains("0000deadbeef"),
            "⚠⚠ a run driven by other code must name it — its walk is evidence about THAT build: \
             {other:?}",
        );

        // ⚠ A run restored from a log written before daemons recorded this. The KEY IS ABSENT, which
        // is how the daemon spells it (omitted, never `null`), so the fixture omits it too.
        let unrecorded = render_build(&json!({ "id": 3 }));
        assert!(
            !unrecorded.is_empty(),
            "⚠⚠⚠⚠⚠ SILENCE HERE WOULD SAY «driven by your build», about a run this build never \
             drove. Absent means nobody recorded it, and the row must say that out loud",
        );
        assert_ne!(
            unrecorded,
            render_build(&json!({ key: sprag_host::wire::BUILD })),
            "⚠⚠⚠ and it must not render the SAME as agreement, or the distinction is only in the \
             source",
        );
    }

    /// ⚠⚠⚠⚠ **A DAEMON WHOSE BINARY WAS REPLACED UNDER IT IS STILL A DAEMON** — the case that broke
    /// `kill-server` at the exact moment it was written for.
    ///
    /// Linux appends ` (deleted)` to `/proc/<pid>/exe` once the running binary's directory entry is
    /// gone. **Building a new daemon is what deletes it**, so every promotion produces exactly this
    /// reading — and the first version of the guard refused it, naming a path ending in `(deleted)`
    /// at a person who was doing the one thing the verb exists for.
    ///
    /// ⚠ Measured on the owner's own daemon rather than imagined, by running the fixed `kill-server`
    /// at the moment it was needed. **A guard that is wrong exactly when it is needed reads as a
    /// broken product**, not as a rule doing its job.
    /// ⚠⚠ THE CASES ARE BUILT FROM [`sprag_rpc::DAEMON_BIN_NAME`] AND FROM DIRECTORIES THAT ARE
    /// OBVIOUSLY NOT ANYBODY'S — the rule is *the file NAME, with the kernel's suffix allowed*, and a
    /// fixture pasting one machine's real paths would state a layout instead. It would also go stale
    /// the day the binary is renamed, silently, by continuing to pass about a name nothing uses.
    #[test]
    fn a_daemon_whose_binary_was_replaced_under_it_is_still_a_daemon() {
        use std::path::Path;

        let daemon = sprag_rpc::DAEMON_BIN_NAME;
        for running in [
            format!("/anywhere/{daemon}"),
            // The promotion's own shape: the binary replaced under the running process.
            format!("/anywhere/else/{daemon} (deleted)"),
            // ⚠ And with no directory at all — a daemon found on `PATH` rather than beside a client.
            daemon.to_owned(),
        ] {
            assert!(
                runs_the_daemon(Path::new(&running)),
                "⚠⚠⚠ {running:?} IS the daemon, and refusing it means refusing to stop the very \
                 process a promotion has just replaced",
            );
        }

        // ── AND THE GUARD STILL GUARDS, or the strip above has swallowed the rule ──
        for other in [
            // The measured failure the guard exists for: a test harness serving its own socket.
            "/anywhere/deps/cli-0123456789abcdef".to_owned(),
            "/anywhere/python3".to_owned(),
            // ⚠ THE NEAREST MISS THERE IS — the CLIENT, whose name is a PREFIX of the daemon's. A
            // strip that took more than the exact suffix, or a `starts_with`, turns one into the
            // other and `kill-server` starts signalling whatever ran the command.
            "/anywhere/sprag".to_owned(),
            "/anywhere/sprag (deleted)".to_owned(),
            // ⚠ A name that merely CONTAINS the daemon's, which a `contains` check would admit.
            format!("/anywhere/not-{daemon}"),
            // ⚠ And the suffix without the name, so the strip cannot be what decides.
            "/anywhere/something (deleted)".to_owned(),
        ] {
            assert!(
                !runs_the_daemon(Path::new(&other)),
                "⚠⚠⚠ {other:?} is NOT a sprag daemon, and signalling it would end a process nobody \
                 asked to end — which is what the first draft of `kill-server` did to this very \
                 test suite",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **THE SEAM, MEASURED ON WHATEVER KERNEL THIS IS** — the half the table above cannot
    /// reach, because every one of its rows is a path a person typed.
    ///
    /// [`runs_the_daemon`] was fully gated in both directions and entirely green on macOS while the
    /// guard around it let every pid through, since nothing there ever CALLED it: the old fallback
    /// asked *"is this pid my own process?"*, which is false of every real invocation. **A rule can
    /// be right in a function nothing reaches** — register item 482's finding, one crate over.
    ///
    /// So this asks the kernel about a process whose identity is known independently: THIS one.
    #[test]
    fn this_kernel_says_what_a_pid_is_running() {
        let mine = libc::pid_t::try_from(std::process::id()).expect("a pid fits in a pid_t");
        let read = exe_of(mine).expect("this process is running, whatever else is true");
        let known = std::env::current_exe().expect("a test binary knows its own path");

        assert_eq!(
            read.file_name(),
            known.file_name(),
            "⚠⚠⚠ this kernel's answer for THIS process ({read:?}) is not the binary running it \
             ({known:?}), so `a_daemon` is deciding on something other than what a pid is running",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE DEFECT ITSELF — A THIRD PROCESS, WHICH IS THE ONLY SHAPE THAT TELLS THE TWO
    /// GUARDS APART.** Register item 487(c).
    ///
    /// The old guard's non-Linux arm refused only *this very process*, so a gate that asked about
    /// its own pid passed on both kernels and proved nothing. A CHILD is neither this process nor a
    /// daemon — and on macOS the old arm waved it straight through, which is `kill-server`
    /// SIGTERM-ing whatever happens to serve a host socket.
    ///
    /// ⚠ The program is asked of the MACHINE rather than spelled (`/bin/sleep` does not exist on
    /// every unix — register item 472, measured on this same runner).
    #[test]
    fn a_third_process_that_is_not_the_daemon_is_refused_however_this_kernel_spells_it() {
        let mut child = std::process::Command::new(sprag_gate::doubles::system("sleep"))
            .arg("30")
            .spawn()
            .expect("this machine can run its own `sleep`");
        let pid = libc::pid_t::try_from(child.id()).expect("a pid fits in a pid_t");

        let refused = a_daemon(pid);
        let _ = child.kill();
        let _ = child.wait();

        let why = refused.expect_err(
            "⚠⚠⚠⚠⚠ A PROCESS THAT IS NEITHER THIS ONE NOR THE DAEMON WAS WAVED THROUGH, and what \
             comes next is a SIGTERM. On 2026-08-20 that killed `sprag-host`'s own `cli` harness \
             on macOS and the suite could not even report it — the exit status was the only witness",
        );
        assert_eq!(
            why.kind(),
            io::ErrorKind::PermissionDenied,
            "a refusal to signal is a permission answer, not a lookup failure: {why}",
        );
        assert!(
            why.to_string().contains(sprag_rpc::DAEMON_BIN_NAME),
            "and it must name what it WAS looking for, so a person can tell this from a daemon \
             that is merely absent: {why}",
        );
    }

    /// The measured permission dialog, as a blocked run carries it.
    fn asked_dialog() -> sprag_detect::Question {
        sprag_detect::Question {
            asked: vec!["Do you want to proceed?".to_owned()],
            choices: vec![
                sprag_detect::Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                sprag_detect::Choice {
                    number: 2,
                    label: "No, and tell me why".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// A finished run's entry as `query("runs")` renders it — built by the DAEMON's own renderer
    /// from a real [`sprag_plugin::Outcome`], never from hand-written JSON.
    ///
    /// ⚠ That is the point of the helper rather than an economy. A fixture that spelled the answer
    /// shape itself would pass while the daemon published something else — the two-readers defect
    /// this workspace keeps paying for, reintroduced inside the gate meant to catch it.
    fn run_entry(outcome: &sprag_plugin::Outcome) -> Value {
        serde_json::json!({
            "id": 7,
            "label": "orchestrator pane=1",
            "state": {
                "status": "done",
                "outcome": sprag_host::plugins::outcome_to_json(outcome),
                "output": Value::Null,
            },
        })
    }

    /// ⛔⛔⛔⛔⛔ **THE ROW SAYS WHICH RUN IT IS, BESIDE THE NUMBER THAT CANNOT** — register item
    /// 887, at the mouth a watcher copies from.
    ///
    /// # ⛔⛔⛔⛔⛔ What the number was being read as
    ///
    /// The loop's own watcher names its log file `run<N>.log` and every table this repository has
    /// built about itself joins that file to a row by `N`. Measured 2026-09-04 in this daemon's
    /// state: rows 199, 200 and 202 each name a run that began after the log bearing that number
    /// had already been finished by a different run — `RunRegistry::restore` sets `next_id` from
    /// the rows it FINDS, so a log that lost rows reissues numbers a predecessor spent.
    ///
    /// ⇒ The thing that CAN identify the run has to arrive in the same glance as the thing that
    /// cannot, or a watcher goes on recording the number alone.
    ///
    /// # ⚠⚠⚠ The absence is a SENTENCE and not an omission
    ///
    /// `render_build`'s call one field over, and here it decides more: a row out of a log written
    /// before the stamp existed is not *the same run as whatever else bears this number*, it is
    /// nobody having recorded which run it was. A silent omission would read as the former to
    /// anyone comparing two rows, which is the reassuring reading of an unmeasured value.
    #[test]
    fn the_row_says_which_run_it_is_beside_the_number_that_cannot() {
        const MINE: &str = "1f4a-17e2c9d31bb40000-0.c7";

        let mut run = run_entry(&a_run_that_closed(None));
        run[sprag_host::plugins::RUN_WHICH_RUN_KEY] = serde_json::json!(MINE);
        let said = render_run(&run);
        assert!(
            said.contains(MINE),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 887: the daemon publishes which run this is and `sprag runs` \
             does not print it, so a watcher keying its records on the number has nothing better \
             offered — and that number names two runs. This is the exact failure `render_run`'s \
             own comment warns about:\n{said}",
        );
        let head = said.lines().next().expect("a row has a heading");
        assert!(
            head.contains(MINE),
            "⚠⚠⚠ AND ON THE HEAD LINE, where the number it qualifies is. A stamp printed further \
             down is one a watcher copying `run {{id}}` never sees: {head:?}",
        );

        // ── AND A RUN NOBODY STAMPED SAYS SO ──
        let unstamped = render_run(&run_entry(&a_run_that_closed(None)));
        assert!(
            unstamped.contains("which run not recorded"),
            "⛔⛔⛔⛔ REGISTER ITEM 887: a row out of a log written before the stamp existed is \
             SILENT about it, so it reads exactly like a row whose stamp happens to match. \
             *Nobody recorded which run this was* is a third answer and the mouth must say \
             it:\n{unstamped}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE ROW A PERSON READS SAYS WHICH REPOSITORY THE RUN WAS FOR** — register item
    /// 890, at the mouth a watcher copies from.
    ///
    /// # ⛔⛔⛔⛔⛔ One daemon drives three trees and this row named none of them
    ///
    /// Measured 2026-09-04 on this daemon's own store: **211 rows, 2 carrying a `request`** — the
    /// two still running — and the repository appeared nowhere else, so **209 rows could not say
    /// which of three repositories they belonged to**. Even the live two named one only inside the
    /// PROSE of their `north_star`.
    ///
    /// # ⚠⚠⚠ THE POPULATION IS TWO REPOSITORIES, WHICH IS THE ITEM'S OWN DONE-WHEN
    ///
    /// A fixture holding one tree cannot fail: *unknown* and *right* are the same value there, and
    /// a renderer printing a fixed string — the daemon's cwd, the first row's tree, anything —
    /// passes it. So two rows go in and the gate asserts each says **its own**, which is the
    /// question a watcher attributing a daemon's runs actually asks.
    ///
    /// ⚠ Position asserted rather than described, [`render_run`]'s constraint: the outer-loop
    /// watcher takes a run's STATUS from the line after the heading, so this clause belongs ON the
    /// heading and nowhere below it.
    #[test]
    fn the_row_a_person_reads_says_which_repository_the_run_was_for() {
        const MINE: &str = "/home/coin/sprag";
        const ANOTHER: &str = "/home/coin/watching-zenoh";

        let in_tree = |tree: Option<&str>| -> String {
            let mut row = run_entry(&a_run_that_closed(None));
            if let Some(tree) = tree {
                row[sprag_host::plugins::RUN_TREE_KEY] = serde_json::json!(tree);
            }
            render_run(&row)
        };

        // ══ ① EACH ROW NAMES ITS OWN TREE, ON THE HEAD LINE ════════════════════════════════════
        let mine = in_tree(Some(MINE));
        let another = in_tree(Some(ANOTHER));
        for (tree, row) in [(MINE, &mine), (ANOTHER, &another)] {
            let head = row.lines().next().expect("a row has a heading");
            assert!(
                head.contains(tree),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 890: the daemon publishes which repository this run was \
                 for and `sprag runs` does not print it on the head line, so a watcher copying \
                 `run {{id}}` attributes the run to whichever tree they are standing in. Wanted \
                 {tree}: {head:?}",
            );
        }

        // ══ ② AND THE TWO ROWS DIFFER ══════════════════════════════════════════════════════════
        //
        // ⛔⛔⛔ THE ITEM'S OWN DONE-WHEN: *한 저장소짜리 픽스처에서는 미상과 정답이 같은 값이다*.
        // A renderer answering one fixed sentence satisfies ① for a single-tree population and
        // leaves every run of every repository attributed to one — which is the state being
        // repaired, not a repair of it.
        assert_ne!(
            mine.lines().next(),
            another.lines().next(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 890: two runs driven in DIFFERENT repositories render the \
             same head line, so the mouth is printing something other than the run's own tree. \
             That is exactly the failure the column exists to end.\n{mine}\n{another}",
        );

        // ══ ③ AND A RUN NOBODY RECORDED A TREE FOR SAYS SO ═════════════════════════════════════
        //
        // ⚠⚠ Rule 6, and the sharpest case of it in this file: silence here reads as *this run is
        // mine* to whoever is standing in a repository at the time, which is the reassuring
        // reading of an unmeasured value. 209 of 211 rows are this arm today.
        let unrecorded = in_tree(None);
        assert!(
            unrecorded.contains("tree not recorded"),
            "⛔⛔⛔⛔ REGISTER ITEM 890: a row out of a log written before the column existed is \
             SILENT about its repository, so it reads exactly like a row whose tree happens to be \
             the reader's. *Nobody recorded which tree* is a third answer and the mouth must say \
             it:\n{unrecorded}",
        );
        assert!(
            !unrecorded.contains(MINE) && !unrecorded.contains(ANOTHER),
            "⚠⚠ AND IT NAMES NO TREE AT ALL: an absence filled in with a guess is worse than a \
             blank, because a reader acts on it:\n{unrecorded}",
        );

        // ══ ④ AND IT DID NOT PUSH ITEM 887's STAMP OFF THE END OF THE LINE ═════════════════════
        //
        // ⛔⛔⛔⛔⛔ **THE FIRST BUILD OF THIS CLAUSE DID EXACTLY THAT**, with a comment asserting it
        // had not. The repayment skill's `watch.sh` reads the stamp as
        // `sed -n 's/.*\[\([^][]*\)\]$/\1/p'` — **anchored at the end of the line** — so a head line
        // ending in a tree path yields the empty string, the watcher records *which run not
        // recorded*, and item 887's claim (a watcher can tell a reissued number from its own run)
        // is off with nothing to show for it. Measured by running that expression over both orders.
        //
        // ⚠⚠ Asserted as *the head ends with the bracketed stamp*, which is that reader's pattern
        // reduced to what it actually requires — and it is asserted with BOTH clauses present,
        // because either alone leaves the other free to move.
        const STAMP: &str = "1f4a-17e2c9d31bb40000-0.c7";
        let mut both = run_entry(&a_run_that_closed(None));
        both[sprag_host::plugins::RUN_TREE_KEY] = serde_json::json!(MINE);
        both[sprag_host::plugins::RUN_WHICH_RUN_KEY] = serde_json::json!(STAMP);
        let head = render_run(&both)
            .lines()
            .next()
            .expect("a row has a heading")
            .to_owned();
        assert!(
            head.contains(MINE),
            "⚠ THE PREMISE: this arm needs BOTH clauses on the head, or it is measuring the stamp \
             alone: {head:?}",
        );
        assert!(
            head.ends_with(&format!("[{STAMP}]")),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 890 BROKE REGISTER ITEM 887's READER: the head line no longer \
             ENDS with the bracketed stamp, and the repayment skill's watcher extracts it with a \
             pattern anchored there. It will read *which run not recorded* for every run, so a \
             reissued number stops being detectable — silently, because the stamp is still printed \
             and still looks right to a person. Put this item's clause BEFORE the stamp: {head:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE ROW A PERSON READS SAYS WHO ASKED FOR THE RUN, WHATEVER THE ANSWER IS** —
    /// register item 865 at the one mouth that is not an agent's.
    ///
    /// # ⚠⚠⚠⚠⚠ Silence was the defect, and it was silence about a product-wide gap
    ///
    /// A promotion kills every run a daemon drives, so somebody has to find each run's owner first.
    /// This row's answer was `pane=786` — where the run TYPES — and no clause at all when nothing
    /// was recorded. Item 865 measured what that cost: three sessions messaged, five messages,
    /// forty minutes, and the owner alive and reachable throughout.
    ///
    /// **Measured on the live loop daemon while this gate was written: 190 of 190 runs carried no
    /// conversation**, because the CLI door that launches them sends no opener. A row that printed
    /// nothing made that look like a per-run coincidence instead of a door with no mouth. ⚠ Do not
    /// carry the count — re-derive the predicate:
    /// `jq '[.runs[] | select(.opened_by_session != null)] | length'` over the daemon's `*.runs.json`.
    ///
    /// # ⚠⚠⚠ The three rows must DIFFER, or this gate measures nothing
    ///
    /// *Named*, *seat only*, and *nothing recorded* are three different things to do next — go and
    /// ask; go and look at that pane knowing it is a guess; ask the person. A renderer appending one
    /// fixed sentence would satisfy *the clause is never empty* and leave all three exactly as
    /// indistinguishable as it found them, which is the state item 865 was opened in.
    ///
    /// ⚠⚠ **AND THE SEATLESS NAMED ROW IS THE ONE THE ITEM IS ABOUT**: a conversation recorded with
    /// no pane holding it. That is the promotion case, and a mouth that only spoke when a seat was
    /// present would be silent exactly when it is needed.
    ///
    /// ⚠ Position asserted, not described — `render_run`'s constraint: the outer-loop watcher reads
    /// the STATUS as the line after the heading, so this clause belongs ON the heading.
    #[test]
    fn the_row_a_person_reads_says_who_asked_for_the_run() {
        const ASKER: &str = "pinion-66";

        let asked_by = |session: Option<&str>, seat: Option<u64>| -> String {
            let mut row = run_entry(&a_run_that_closed(None));
            if let Some(session) = session {
                row[sprag_host::plugins::RUN_ASKED_BY_KEY] = serde_json::json!(session);
            }
            if let Some(seat) = seat {
                row[sprag_host::plugins::RUN_OPENED_BY_KEY] = serde_json::json!(seat);
            }
            render_run(&row)
        };

        let nobody = asked_by(None, None);
        let seat_only = asked_by(None, Some(786));
        let named_seatless = asked_by(Some(ASKER), None);
        let named_seated = asked_by(Some(ASKER), Some(786));

        // ── NEVER SILENT ────────────────────────────────────────────────────────────────────────
        for (what, row) in [
            ("nobody recorded", &nobody),
            ("a seat and no name", &seat_only),
            ("a name and no seat", &named_seatless),
            ("a name and a seat", &named_seated),
        ] {
            assert!(
                row.lines().next().unwrap_or_default().contains("asked for"),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 865 ⑴: every run row says who asked, INCLUDING when the \
                 answer is that nobody wrote it down — *「모른다」도 답이다*. Until this, a run with \
                 a recorded owner and a run with none produced identical rows, so a person could \
                 not tell whether asking would help. This row is {what}: {row}",
            );
        }

        // ── AND THE FOUR ANSWERS ARE FOUR ANSWERS ───────────────────────────────────────────────
        assert!(
            named_seatless.contains(ASKER) && named_seated.contains(ASKER),
            "⛔⛔⛔⛔⛔ THE NAME ITSELF, verbatim, because it is what a person types to reach the \
             owner. Read {named_seatless} and {named_seated}",
        );
        assert!(
            !seat_only.contains(ASKER) && !nobody.contains(ASKER),
            "⚠⚠ THE CONTROL: a row with no conversation recorded must not invent one. Without this \
             a mouth printing a constant would pass every claim here: {seat_only} / {nobody}",
        );
        assert!(
            seat_only.contains("786") && seat_only.contains("guess"),
            "⚠⚠⚠ A SEAT WITHOUT A NAME IS A GUESS AND THE ROW SAYS SO. Item 865's own measurement \
             is that neither cwd nor session age tells one occupant of a pane from another, so a \
             bare pane number reads as an owner and is not one: {seat_only}",
        );
        assert_ne!(
            named_seatless, named_seated,
            "⚠⚠⚠⚠ THE SEATLESS NAMED ROW IS THE ONE ITEM 865 WAS OPENED FOR — the asker's pane \
             closed or their session moved, `seat_of` answers None, and before this the row went \
             silent about a run whose owner was recorded and reachable. If these two are identical \
             the row is not saying that the pane is gone: {named_seatless}",
        );
        assert!(
            named_seatless
                .lines()
                .next()
                .unwrap_or_default()
                .contains("no longer here"),
            "⚠⚠⚠ ...and it says which way it is gone, because *ask this conversation* and *look in \
             this pane* are different next steps: {named_seatless}",
        );

        // ── ON THE HEADING, where `render_run`'s constraint puts it ─────────────────────────────
        assert!(
            !named_seated
                .lines()
                .skip(1)
                .any(|line| line.contains("asked for by")),
            "⚠⚠ `render_run`'s constraint as a predicate: this clause belongs on the heading, where \
             the pane clause it replaces already was. A detail line here would move the outer-loop \
             watcher, which reads the STATUS as the line right after the heading: {named_seated}",
        );
    }

    /// A run that stopped on its peer's question, refused for `why`.
    fn blocked_run(why: sprag_plugin::Refusal, answered: u32) -> sprag_plugin::Outcome {
        sprag_plugin::Outcome {
            state: sprag_plugin::OutcomeState::Blocked(Some(match why {
                sprag_plugin::Refusal::Unreadable => sprag_plugin::Unanswered::unreadable(),
                other => sprag_plugin::Unanswered::refused(asked_dialog(), other),
            })),
            iterations: 2,
            cost: Some(sprag_plugin::Cost::Bytes(14)),
            failure: None,
            stopped: None,
            answered,
            screened: 0,
            deferred: None,
            unchecked: None,
            unadmitted: None,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            // ⚠ `None` and not a zero: this fixture is not a run that counted nothing, it is one
            // that does not count — the distinction `Banked` exists to keep.
            banked: None,
            // ⚠ `None` on `banked`'s terms: this fixture is not a run briefed with nothing, it is
            // one nobody briefs — the distinction `Briefing` keeps.
            briefed: None,
            // ⚠ A run BLOCKED on its peer's question has not ended on its own terms, so no ending
            // is named — see `sprag_plugin::Outcome::done_reason`.
            done_reason: None,
        }
    }

    /// A run that closed on its own terms, naming `ending` — or naming none when that is [`None`].
    ///
    /// ⚠ `Converged` in every arm on purpose: that is the whole premise of the gate below, and a
    /// fixture free to vary the state word could pass it while `state` was still doing the work.
    fn a_run_that_closed(ending: Option<&'static str>) -> sprag_plugin::Outcome {
        sprag_plugin::Outcome {
            state: sprag_plugin::OutcomeState::Converged,
            iterations: 9,
            cost: Some(sprag_plugin::Cost::Bytes(31)),
            failure: None,
            stopped: None,
            answered: 0,
            screened: 0,
            deferred: None,
            unchecked: None,
            unadmitted: None,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: None,
            briefed: None,
            done_reason: ending.map(std::borrow::Cow::Borrowed),
        }
    }

    /// ⛔⛔⛔⛔⛔ **THE ROW A PERSON READS SAYS HOW FULL THE SESSION GOT AGAINST ITS BOUND** —
    /// register items 894 and 856(1b), at the mouth that is not an agent's.
    ///
    /// # ⛔⛔⛔⛔⛔ A fact that reaches the wire and dies at the mouth
    ///
    /// This file names that failure in five places, and the composer having a gate is not the same
    /// as the mouth printing what it composes: the clause is one interpolation in a format string,
    /// deleting it leaves `context_sentence`'s own gate perfectly green, and item 856's whole
    /// remaining debt is *somebody reads the number*. A value that stops one format string short
    /// of the reader pays nothing at all.
    ///
    /// ⚠⚠ **TWO ROWS, because one cannot fail.** A renderer printing a fixed sentence — or the
    /// ceiling twice, or the reading twice — satisfies a single-row fixture. The two here differ
    /// in the SHARE and in nothing else a reader would notice, which is the number the pair exists
    /// to produce: 612,000 is nearly full under an 800,000 ceiling and long past a 100,000 one.
    ///
    /// ⚠ And a run that reported neither says nothing rather than a zero — rule 6, and register
    /// item 891 one field over.
    #[test]
    fn the_row_a_person_reads_says_how_full_the_session_got_against_its_bound() {
        let with = |fullest: Option<i64>, ceiling: Option<i64>| -> String {
            let mut row = run_entry(&a_run_that_closed(None));
            if let Some(fullest) = fullest {
                row[sprag_host::plugins::RUN_CONTEXT_HIGH_WATER_KEY] = serde_json::json!(fullest);
            }
            if let Some(ceiling) = ceiling {
                row[sprag_host::plugins::RUN_CONTEXT_CEILING_KEY] = serde_json::json!(ceiling);
            }
            render_run(&row)
        };

        // ══ ① THE ROW SAYS IT, AND THE TWO CEILINGS READ DIFFERENTLY ═══════════════════════════
        let roomy = with(Some(612_000), Some(800_000));
        let tight = with(Some(612_000), Some(100_000));
        for (share, row) in [("76 %", &roomy), ("612 %", &tight)] {
            assert!(
                row.contains("612000") && row.contains(share),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 894: the daemon publishes how full a run's session got \
                 and `sprag runs` does not print it, so the only reader item 856 has is back to \
                 the bound alone. Wanted {share}: {row:?}",
            );
        }
        assert_ne!(
            roomy, tight,
            "⛔⛔⛔⛔ REGISTER ITEM 894: the same reading under two ceilings renders identically, \
             so the mouth is printing something other than the comparison. The share IS the \
             answer — a run at 76 % of its window and one at six times it want opposite \
             remedies:\n{roomy}\n{tight}",
        );

        // ══ ② AND A RUN THAT REPORTED NEITHER SAYS NOTHING, never a zero ═══════════════════════
        let silent = with(None, None);
        assert!(
            !silent.contains("fullest its session"),
            "⛔⛔⛔ RULE 6, and register item 891 one field over: a row out of a log written before \
             these columns existed must stay SILENT. A `0 %` printed for silence reads as a run \
             that never used its window, which is the reassuring reading of an unmeasured \
             value:\n{silent}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE ROW A PERSON READS SAYS WHICH OF THE RUN'S NUMBERS WERE NOT ITS
    /// DOCUMENT'S** — register item 859(2), at the mouth that is not an agent's.
    ///
    /// # ⛔⛔⛔⛔⛔ The key has been on the row since item 853 and this mouth never printed it
    ///
    /// Measured 2026-09-05: `render_run` interpolated nine clauses about a run and `overridden`
    /// was in none of them, so the only way a person learned the answer existed was to read the
    /// JSON or this source. That is item 859(2) word for word — *one path by which a person knows
    /// the key is there before they go looking at a row* — and it is what made item 856's two
    /// experiment arms tellable from its seventeen ordinary runs only by a human note.
    ///
    /// # ⛔⛔⛔ THREE rows, because the third is the one a list cannot carry
    ///
    /// A run whose caller took numbers, one whose caller took none, and one whose document
    /// authored none. The middle is the AFFIRMATIVE — *this run obeyed its own document* — and a
    /// renderer that printed nothing for it would make it indistinguishable from the third, which
    /// is `Overridden::joined`'s whole rule arriving at a reader.
    #[test]
    fn the_row_a_person_reads_says_which_of_its_numbers_were_not_its_documents() {
        let with = |took: Option<Vec<&str>>| -> String {
            let mut row = run_entry(&a_run_that_closed(None));
            if let Some(took) = took {
                row[sprag_host::plugins::RUN_OVERRIDDEN_KEY] = serde_json::json!(took);
            }
            render_run(&row)
        };

        // ══ ① THE ROW NAMES WHAT THE CALLER TOOK ═══════════════════════════════════════════════
        let taken = with(Some(vec!["context_ceiling", "max_seconds"]));
        assert!(
            taken.contains("context_ceiling") && taken.contains("max_seconds"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 859(2): the daemon has published which numbers a caller took \
             since item 853 and `sprag runs` prints none of them, so a person auditing a run \
             cannot see that its ceiling was never its document's. Item 856's arms were separated \
             from its baseline by hand for exactly this: {taken:?}",
        );

        // ══ ② AND THE HEALTHY LAUNCH IS SAID OUT LOUD ══════════════════════════════════════════
        let clean = with(Some(Vec::new()));
        let unauthored = with(None);
        assert!(
            clean.contains("its own document's"),
            "⛔⛔⛔⛔ REGISTER ITEM 859(2): an EMPTY list is the affirmative — *this run ran under \
             every number its own document set* — and it is the reading item 853 was filed \
             because nobody could get. A renderer silent about it publishes the healthy launch as \
             nothing: {clean:?}",
        );
        assert_ne!(
            clean, unauthored,
            "⛔⛔⛔⛔⛔ RULE 6: *its document authored numbers and the caller took none* and *its \
             document authored none* must not render alike. The first is a claim about this run's \
             obedience and the second is a claim about its plugin, and a reader who cannot tell \
             them apart will read the second as the first — which is how a run spending under \
             somebody else's numbers reads as compliant:\n{clean}\n{unauthored}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE ROW A PERSON READS SAYS WHICH ENDING CLOSED THE RUN** — register item 706's
    /// third requirement at the one mouth that is not an agent's, and register item 594's collapse
    /// arriving one surface over.
    ///
    /// # ⚠⚠⚠⚠⚠ The wire gained the word and this mouth dropped it
    ///
    /// `RUN_DONE_REASON_KEY` puts the ending on the row, and `list_runs` hands it to an agent. But
    /// `sprag runs` has **no machine form** — `render_run` is its only mouth — so a person had the
    /// word available nowhere at all. *A fact that reaches the wire and dies at the mouth somebody
    /// actually reads* is the sentence this file has already written twice, for items 594 and 685.
    ///
    /// # ⚠⚠⚠ The two rows have to DIFFER, or this gate measures nothing
    ///
    /// `stood_down` already had a sentence here, and it appears only when somebody GAVE an order.
    /// So the pair that proves the point is `declared` against `no_successor`: both converge,
    /// neither was ordered, and before this clause their rows were byte-identical. A renderer that
    /// appended one fixed sentence to every converged run would satisfy *the word is not alone* and
    /// leave that pair exactly as indistinguishable as it found them.
    ///
    /// ⚠⚠ **AND THE CONTROL IS THE ABSENCE**: a run that named no ending must gain no clause, or a
    /// mouth printing unconditionally would pass both arms above.
    ///
    /// # ⚠ Why the POSITION is asserted rather than described
    ///
    /// `render_run`'s own comment records the constraint every detail clause here is written under:
    /// this repository's outer-loop watcher reads a run's STATUS as the line right after the
    /// heading and its walk as the block's LAST line, so a clause landing at either end silently
    /// moves a reader that already exists. That was prose nobody could re-run. Here it is a
    /// predicate: the ending's clause is neither of those two lines.
    #[test]
    fn the_row_a_person_reads_says_which_ending_closed_the_run() {
        const CLAUSE: &str = "it closed under";

        let quiet = render_run(&run_entry(&a_run_that_closed(None)));
        assert!(
            !quiet.contains(CLAUSE),
            "⚠⚠⚠⚠⚠ THE CONTROL: a run that named no ending must gain no clause — absence is how \
             this row says *nobody named one*. Without this the assertions below would pass on a \
             mouth that printed a sentence for every converged run: {quiet}",
        );

        let declared = render_run(&run_entry(&a_run_that_closed(Some("declared"))));
        let no_successor = render_run(&run_entry(&a_run_that_closed(Some("no_successor"))));
        assert!(
            declared.contains("`declared`") && no_successor.contains("`no_successor`"),
            "⛔⛔⛔⛔⛔ ITEM 706 ③ AT THE MOUTH: each row must carry its own ending's word, \
             verbatim as the plugin spelled it. Read {declared} and {no_successor}",
        );
        assert_ne!(
            declared, no_successor,
            "⛔⛔⛔⛔⛔ ITEM 594 ONE SURFACE OVER: these two runs both converged and neither was \
             ordered to stand down, so before this clause their rows were identical — a person \
             could not tell an agent claiming the north star reached from a reflection that ran \
             out of things to propose. Read {declared}",
        );

        // ── AND IT LANDS WHERE THE WATCHER'S TWO POSITIONAL READS ARE NOT ──
        let lines: Vec<&str> = declared.trim_end().lines().collect();
        let carrying = lines
            .iter()
            .position(|line| line.contains(CLAUSE))
            .expect("the clause is on the row the assertion above just read");
        assert!(
            carrying > 1 && carrying + 1 < lines.len(),
            "⚠⚠⚠⚠ `render_run`'s constraint, as a predicate rather than a paragraph: the \
             outer-loop watcher reads the STATUS as the line after the heading and the walk as the \
             LAST line, so a clause at either end moves a reader that already exists. This one is \
             at line {carrying} of {}: {declared}",
            lines.len(),
        );
    }

    /// A row as the daemon publishes one whose driver died saying `why`.
    ///
    /// ⚠⚠ The KEY comes from the host — `run_to_json` is private, so this cannot go through it, and
    /// a literal here would make the fixture a second author of the very word item 685 is about.
    /// `a_panicked_runs_reason_reaches_the_wire` over there pins that the daemon fills it.
    fn a_driver_that_died(why: &str) -> Value {
        serde_json::json!({
            "id": 12,
            "label": "ai_loop pane=3",
            "state": {
                "status": sprag_host::plugins::RunStatus::Panicked.wire_str(),
                sprag_host::plugins::RUN_ERROR_KEY: why,
            },
        })
    }

    /// ⛔⛔⛔⛔⛔ **A `panicked` RUN TELLS A PERSON WHAT KILLED ITS DRIVER** — register item 685, and
    /// register item 594's twin one word over.
    ///
    /// # ⚠⚠⚠⚠⚠ The word that reads as an accusation
    ///
    /// Filed 2026-08-25 by another repository's watcher, which paid for it: two of its runs ended
    /// `panicked` and it read the word as *my run hit a bug*. A `kill-server` had killed the
    /// drivers. The difference was **already on the wire** — `RunState::Panicked` carries the
    /// sentence `driver_ending` composed, which names the exit status and so names the signal — and
    /// this mouth printed the status word alone. *A fact that reaches the wire and dies at the
    /// mouth somebody actually reads* is the sentence the `Reported` arm beside it already wrote.
    ///
    /// # ⚠⚠⚠ The two rows have to say DIFFERENT things, or this gate measures nothing
    ///
    /// A renderer that appended one fixed clause to every `panicked` run would satisfy *the word is
    /// not alone* and still leave the reader unable to tell a signal from a bug — which is the
    /// whole of what the item is about. So the claim is a DIFFERENCE, driven with two reasons that
    /// a real daemon composes differently.
    ///
    /// ⚠ The third arm is the control: `interrupted` comes through the same match arm and carries
    /// no such key, so it must gain no clause at all. Without it, a mouth that printed a sentence
    /// unconditionally would pass the two arms above.
    #[test]
    fn a_panicked_run_tells_a_person_what_killed_its_driver() {
        let signalled = render_run(&a_driver_that_died(
            "a run's driver process ended signal: 9 (SIGKILL) without reporting an outcome",
        ));
        let crashed = render_run(&a_driver_that_died(
            "a run's driver process ended exit status: 101 without reporting an outcome: thread \
             'main' panicked at crates/sprag-host/src/drive.rs:1:1",
        ));

        assert!(
            signalled.contains("SIGKILL"),
            "⚠⚠⚠⚠⚠ A RUN WHOSE DRIVER WAS KILLED MUST SAY SO. `panicked` alone is read as *this \
             run hit a bug*, which is what item 685 was filed for: {signalled}",
        );
        assert!(
            crashed.contains("101") && crashed.contains("panicked at"),
            "⚠⚠⚠ and a driver that really crashed must carry ITS reason, not a generic clause: \
             {crashed}",
        );
        assert_ne!(
            signalled, crashed,
            "⚠⚠⚠⚠ AND THE TWO MUST DIFFER, or this gate is vacuous: one fixed sentence appended to \
             every `panicked` run would satisfy both assertions above while leaving the reader \
             exactly where item 685 found them",
        );

        // ── THE CONTROL: the other word through this same arm gains nothing ──
        let parked = render_run(&serde_json::json!({
            "id": 12,
            "label": "ai_loop pane=3",
            "state": { "status": sprag_host::plugins::RunStatus::Interrupted.wire_str() },
        }));
        assert!(
            !parked.contains("without reporting"),
            "⚠⚠ THE CONTROL: `interrupted` carries no reason and must gain no clause — a mouth \
             that printed one unconditionally would pass every assertion above: {parked}",
        );
    }

    /// ⚠⚠⚠ **A BLOCKED RUN SHOWS THE QUESTION, ITS OPTIONS, AND WHICH ONE A BARE ENTER TAKES.**
    ///
    /// This is the mouth a PERSON reads, and a `blocked` run exists to be answered BY that person.
    /// Printing the word alone sent them to go find the pane, read the menu off a screen, and work
    /// out what doing nothing would select — every part of which the daemon had already parsed and
    /// published. Measured before this gate: `render_run` mentioned `asking` nowhere at all, so
    /// R365's whole product died here, at the mouth its own renderer's comment warns about.
    ///
    /// ⚠ The `->` marker is the load-bearing half. On a tool-permission dialog, which option a bare
    /// Enter takes is the difference between confirming a command and declining it, and it is the
    /// one fact a person cannot read off the option text.
    #[test]
    fn a_blocked_run_shows_a_person_the_question_it_wants_answered() {
        let said = render_run(&run_entry(&blocked_run(
            sprag_plugin::Refusal::NoConsent,
            0,
        )));
        assert!(said.contains("blocked"), "{said}");
        assert!(
            said.contains("Do you want to proceed?"),
            "⚠⚠⚠ the QUESTION, or the person has to go and find the pane: {said}",
        );
        assert!(
            said.contains("1. Yes") && said.contains("2. No, and tell me why"),
            "and every option, in the agent's own words: {said}",
        );
        assert!(
            said.contains("-> 1. Yes"),
            "⚠⚠⚠ and WHICH ONE A BARE ENTER TAKES — on a permission dialog that is the difference \
             between confirming a command and declining it: {said}",
        );
        assert!(
            !said.contains("->   2.") && !said.contains("-> 2."),
            "and only that one is marked: {said}",
        );
        assert!(
            said.contains("was given no consent"),
            "⚠⚠ and WHY nothing was answered, as the sentence and not the wire word — `I gave no \
             consent` and `my consent did not fire` are different things to fix: {said}",
        );
    }

    /// ⛔⛔⛔⛔ **A CANCELLED RUN TELLS THE PERSON WHO CANCELLED IT** — register item 596, held at
    /// the MOUTH and not only on the wire.
    ///
    /// # Why the mouth needs its own gate when the key is already gated
    ///
    /// `render_run`'s own comment names the failure it is guarding against — *"a fact that reaches
    /// the wire and dies at the mouth somebody actually reads"* — and until this gate, **nothing in
    /// this binary held it to that**. The host's gates prove the key is published; a renderer that
    /// simply never read it would leave every one of them green while the person typing `sprag
    /// runs` learned nothing, which is the exact shape register item 594 was filed about.
    ///
    /// ⚠⚠⚠ **THE CONTROL IS THE SAME RUN WITH NO CANCEL**, so a renderer that printed a fixed
    /// clause on every run would fail here rather than pass by accident.
    #[test]
    fn a_cancelled_run_tells_the_person_who_cancelled_it() {
        let mut swept = run_entry(&sprag_plugin::Outcome {
            state: sprag_plugin::OutcomeState::Cancelled,
            ..blocked_run(sprag_plugin::Refusal::NoConsent, 0)
        });
        // ⚠⚠⚠ THE RUN HAS A WALK, and giving it one is not decoration — it is what makes the
        // positional assertions below mean anything. A block whose journal is empty ENDS on
        // whatever clause came last, so a fixture without one cannot tell a clause that displaced
        // the walk from a clause that merely followed it. The first run of this gate failed
        // exactly there, and the fixture was the wrong half.
        swept[sprag_host::plugins::RUN_JOURNAL_KEY] = serde_json::json!([{
            "iteration": 1,
            "cost": 245,
            "unit": "bytes",
            "verdict": "continue",
            "note": "Idle --Start--> Priming",
        }]);
        // ⚠ THE HOST'S OWN SENTENCE, taken from the renderer that composes it rather than typed
        // out here: two mouths reading one fact must not reach two conclusions, and a gate that
        // spelled the words itself would go green against a mouth printing something else.
        let expected = sprag_host::plugins::cancel_sentence(
            sprag_host::runs::Canceller::Shutdown,
            &sprag_host::runs::RunState::Done {
                outcome: Box::new(sprag_plugin::Outcome {
                    state: sprag_plugin::OutcomeState::Cancelled,
                    ..blocked_run(sprag_plugin::Refusal::NoConsent, 0)
                }),
                output: None,
                uncommitted: None,
            },
        );
        let quiet = render_run(&swept);
        assert!(
            !quiet.contains("bring the daemon back"),
            "⚠⚠⚠ THE CONTROL: a run with no such key must say nothing about a canceller. A clause \
             printed unconditionally would make the assertion below pass while saying nothing \
             about anybody's cancel: {quiet}",
        );

        swept[sprag_host::plugins::RUN_CANCELLED_BY_KEY] = Value::String(expected.clone());
        let said = render_run(&swept);
        assert!(
            said.contains(&expected),
            "⛔⛔⛔ ITEM 596: the daemon knows a shutdown swept this run and the person reading \
             `sprag runs` is told only `cancelled` — which is the word they cannot act on. The \
             remedy for a swept run is *start it again*, and for a cancelled one it is *ask \
             whoever stopped it*; a mouth that drops this makes them the same line. Got: {said}",
        );
        // ⚠⚠ AND NOT ON THE HEADING LINE, NOR AS THE LAST ONE. This repository's own outer-loop
        // watcher reads a run's STATUS as the line after the heading and its walk as the block's
        // last line, so a clause at either end silently moves a reader that already exists —
        // `render_run`'s comment states the constraint and nothing was holding it.
        let lines: Vec<&str> = said.lines().collect();
        assert!(
            !lines[0].contains("bring the daemon back") && !lines[1].contains("bring the daemon"),
            "⚠⚠⚠ the clause must not land on the heading or the status line under it — the \
             repayment loop's watcher reads both positionally: {said}",
        );
        assert!(
            !lines[lines.len() - 1].contains("bring the daemon back"),
            "⚠⚠⚠ nor last, which is where that watcher reads the walk: {said}",
        );
        assert!(
            lines[lines.len() - 1].contains("Idle --Start--> Priming"),
            "⚠⚠ and the WALK is what is still last, which is the other half of the same claim: a \
             clause that pushed the walk off the end would satisfy the assertion above while \
             breaking the reader it exists to protect: {said}",
        );
    }

    /// ⛔⛔⛔ **A RUN WHOSE PROMPT IS SITTING IN A COMPOSER IS TOLD SO, AND ONE THAT SENT NOTHING IS
    /// NOT** — register item 617, measured against a live `claude` 2026-08-23.
    ///
    /// # ⚠⚠⚠⚠⚠ The run register item 591 exists for was the one saying nothing
    ///
    /// `delivery_sentence` returns early on a zero denominator, which is right for a run that never
    /// typed a byte — and it was also every run whose prompt was typed, painted, and never asked,
    /// because until item 617 nothing counted those. So the wedged run — the one whose whole
    /// symptom is *your prompt is on that pane and nobody was asked it* — printed no delivery line
    /// at all, on the mouth a person greps. MEASURED: a 44-column pane, a 10599-byte brief, the
    /// run's own failure sentence saying the text had been read back off the screen, and its
    /// counters saying `0 of 0`.
    ///
    /// ⚠⚠ **THE CONTROL IS THE SAME RUN WITH THE COUNT AT ZERO**, and it carries the claim: a
    /// sentence printed for a run that delivered nothing would be telling a person to go and look
    /// at a pane no text ever reached — the reassuring wrong answer, and the direction this
    /// repository has paid for twice.
    ///
    /// ⚠ The remedy is asserted rather than the number: what a reader does about a prompt sitting
    /// in a composer is GO AND LOOK, which is the opposite of what `folded` tells them, so the
    /// words are the artefact and the pair of integers is not.
    #[test]
    fn a_run_whose_prompt_never_became_a_question_says_where_that_prompt_is() {
        let wedged = json!({
            "id": 4,
            "state": "failed",
            sprag_host::plugins::RUN_DELIVERED_KEY: 0,
            sprag_host::plugins::RUN_FOLDED_KEY: 0,
            sprag_host::plugins::RUN_UNSUBMITTED_KEY: 1,
        });
        let said = sprag_host::plugins::delivery_sentence(&wedged).unwrap_or_default();
        assert!(
            said.contains("never asked") && said.contains("go and look at that pane"),
            "⛔⛔⛔ ITEM 617: this run typed a prompt onto its pane, watched it painted there, and \
             never got it asked — and the mouth a person reads says nothing about delivery at all. \
             That is the run register item 591 built these counters for. Got: {said:?}",
        );

        let mut silent = wedged.clone();
        silent[sprag_host::plugins::RUN_UNSUBMITTED_KEY] = json!(0);
        assert_eq!(
            sprag_host::plugins::delivery_sentence(&silent),
            None,
            "⚠⚠⚠⚠⚠ THE CONTROL: this run put nothing on any pane, so there is no prompt for \
             anybody to go and look at. A sentence here would send a person to search a screen the \
             text never reached",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN THAT MOVED PANE SENDS A PERSON TO THE PANE IT IS ON, NOT THE ONE IT WAS
    /// BORN OVER** — register item 726, measured 2026-08-27 on run 18.
    ///
    /// # ⛔⛔ What a person was actually told
    ///
    /// `sprag runs` printed `run 18  ai_loop pane=49 … running` and, directly under it, *"1
    /// prompt(s) reached that pane and were never asked … so go and look at that pane"*. **Pane 49
    /// did not exist.** The run had replaced its inner session hours before
    /// (`Restarting --SessionReplaced--> Resuming`) and was driving 54 — established by
    /// `/proc/<pid>/cwd` rather than by counting panes, because the numbers are the thing under
    /// suspicion. So the mouth built so a person can always see how the loop is turning handed them
    /// an instruction that could not be followed: items 243 and 285 broken at the last inch.
    ///
    /// # ⚠⚠⚠ Why the repair is not *print the label properly*
    ///
    /// `label` is prose composed ONCE, when the run opens, and `RunRecord::label`'s own doc says
    /// identity re-derived from prose is identity that drifts. The live answer already existed:
    /// [`sprag_host::plugins::RUN_DRIVING_KEY`] — register item 540 — written from
    /// `Plugin::driving`, which is asked of the driver every step and never cached. **The fact was
    /// never missing; the mouth did not read it**, which is the shape register item 594 was filed
    /// about and this binary keeps re-learning. A clause that cut `49` out of the name in order to
    /// COMPARE the two would rebuild the derive-it-from-a-name defect item 540 retired, so the row
    /// states the live pane outright and lets a reader see for themselves that the name is older.
    ///
    /// # ⚠⚠ The premise, asserted inside — and where the other half of it lives
    ///
    /// The fixture's name and its live pane must actually DIFFER, or a renderer that printed the
    /// name alone would pass this gate untouched. That the two really do diverge in a live run is
    /// not assumed here either: `sprag_plugin`'s
    /// `a_reflection_replaces_the_session_and_the_new_one_is_told_what_was_learned` drives a REAL
    /// replacement and holds `driving()` to the pane that replaced the old one. This gate is the
    /// far end of that same wire — the end a person reads.
    #[test]
    fn a_run_that_moved_pane_sends_a_person_to_the_pane_it_is_on() {
        /// The pane this run's NAME was composed over, when it opened.
        const BORN_OVER: u64 = 49;
        /// The pane it is driving now, after a session replacement.
        const DRIVING_NOW: u64 = 54;
        assert_ne!(
            BORN_OVER, DRIVING_NOW,
            "⚠⚠⚠⚠⚠ THE PREMISE, and without it every arm below is vacuous: the whole defect is a \
             run whose NAME and whose PANE disagree, so a fixture where the two match would go \
             green against a mouth that prints the name and reads nothing",
        );

        /// A run mid-flight, named over one pane and driving another. ⚠ It is given a WALK for the
        /// reason the neighbouring gate states: a block with no journal ENDS on whatever clause
        /// came last, so without one the positional assertions cannot tell a clause that DISPLACED
        /// the walk from one that merely followed it.
        fn moved_run(driving: Option<u64>) -> Value {
            let mut run = serde_json::json!({
                "id": 18,
                "label": format!("ai_loop pane={BORN_OVER}"),
                "state": {
                    "status": "running",
                    "iterations": 12,
                    "cost": 0,
                    "unit": "steps",
                },
                sprag_host::plugins::RUN_JOURNAL_KEY: [{
                    "iteration": 1,
                    "cost": 245,
                    "unit": "bytes",
                    "verdict": "continue",
                    "note": "Idle --Start--> Priming",
                }],
            });
            if let Some(pane) = driving {
                run[sprag_host::plugins::RUN_DRIVING_KEY] = serde_json::json!(pane);
            }
            run
        }

        // ── THE HEADLINE: the row names the pane the run is on ──
        let said = render_run(&moved_run(Some(DRIVING_NOW)));
        assert!(
            said.contains(&format!("walk to is {DRIVING_NOW}")),
            "⛔⛔⛔⛔⛔ ITEM 726: this run is driving pane {DRIVING_NOW} and the only pane anywhere on \
             its row is {BORN_OVER} — the name it was opened with. A person following this row's \
             own «go and look at that pane» walks to a pane that is not there, and the fact that \
             would have told them otherwise was already on the wire. Got: {said}",
        );

        // ⚠⚠ AND IT LANDS WHERE THE TWO POSITIONAL READERS ARE NOT. This repository's own
        // outer-loop watcher takes a run's STATUS as the line after the heading and its walk as the
        // block's LAST line, so a clause at either end silently moves a reader that already exists.
        let lines: Vec<&str> = said.lines().collect();
        assert!(
            !lines[0].contains("walk to is") && !lines[1].contains("walk to is"),
            "⚠⚠⚠ the clause must not land on the heading or on the status line under it — the \
             repayment loop's watcher reads both by position: {said}",
        );
        assert!(
            lines[lines.len() - 1].contains("Idle --Start--> Priming"),
            "⚠⚠ and the WALK is still last, which is the other half of that claim: a clause that \
             pushed the walk off the end would satisfy the assertion above while breaking the \
             reader it exists to protect: {said}",
        );

        // ── ⚠⚠⚠ THE CONTROL: nothing reported a pane, and the NAME MUST NOT STAND IN FOR ONE ──
        let unreported = render_run(&moved_run(None));
        assert!(
            !unreported.contains("walk to is"),
            "⚠⚠⚠⚠ a run nothing has vouched a pane for must not be given one: {unreported}",
        );
        assert!(
            unreported.contains("nothing has reported which pane"),
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, AND IT IS REGISTER ITEM 709's DISCIPLINE AT THIS MOUTH: an \
             absence read as the old value is the defect above wearing the opposite face. This run \
             is RUNNING and no step has said which pane it drives, so the only number on the row is \
             a name from before any of it happened — and silence here lets a reader take that name \
             for a current answer. Got: {unreported}",
        );

        // ── ⚠ AND A RUN THAT HAS STOPPED SAYS NEITHER THING, so the clause is not noise on the
        // rows nobody has to act on ──
        let finished = render_run(&run_entry(&blocked_run(
            sprag_plugin::Refusal::NotOffered,
            0,
        )));
        assert!(
            !finished.contains("walk to is")
                && !finished.contains("nothing has reported which pane"),
            "⚠⚠ a finished run that never reported a pane is history rather than a question, and a \
             clause on every such row would train a reader to skip the line that matters — the \
             argument `render_answered`'s own gate makes one clause over: {finished}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN TYPING INTO A RESTORED PANE SAYS SO ON THE LINE THE POST-PROMOTION CHECK
    /// READS** — register item 869, done-when ⑵.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the placement is the whole assertion and not a detail of it
    ///
    /// The item is a PROMOTION defect: a restart brings an agent pane back with a resume of the
    /// conversation it was in, and the loop in that pane can no longer shed context, because
    /// replacing its session is how it sheds and the pane it replaces from is already somebody
    /// else's history. Measured over four promotions and three repositories, exception 0.
    ///
    /// The check that would have caught it is four commands, and the one that reads runs is
    /// `sprag runs | grep -E '^run ' -A1` — **the heading and exactly one line after it.** So a
    /// clause under the status is a clause that repairs nothing for the reader this item is about,
    /// which is the same constraint items 755 and 774 each measured at this mouth. Hence the second
    /// arm, and hence it is an assertion rather than a comment.
    #[test]
    fn a_run_driving_a_restored_pane_says_so_where_the_promotion_check_looks() {
        /// The daemon's own sentence, as `revived_pane_now` composes it. The renderer carries it
        /// verbatim, so the fixture is what the wire holds and not a phrasing this file invented.
        const SAID: &str = "⚠ the pane this run is driving (54) came back from a restore";

        fn restored_run(revived: Option<&str>) -> Value {
            let mut run = serde_json::json!({
                "id": 18,
                "label": "ai_loop pane=54",
                "state": {
                    "status": "running",
                    "iterations": 12,
                    "cost": 0,
                    "unit": "steps",
                },
                sprag_host::plugins::RUN_JOURNAL_KEY: [{
                    "iteration": 1,
                    "cost": 245,
                    "unit": "bytes",
                    "verdict": "continue",
                    "note": "Idle --Start--> Priming",
                }],
            });
            if let Some(said) = revived {
                run["state"][sprag_host::plugins::RUN_REVIVED_PANE_KEY] = serde_json::json!(said);
            }
            run
        }

        // ── ① THE HEADLINE: the row carries the daemon's sentence ──
        let said = render_run(&restored_run(Some(SAID)));
        assert!(
            said.contains(SAID),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 869: the daemon can see that this run's pane was revived and \
             `sprag runs` prints nothing about it, so the fact dies at the mouth a person reads — \
             the failure this file has now recorded six times. Got: {said}",
        );

        // ── ② AND ON THE STATUS LINE, WHICH IS THE ONLY LINE THE CHECK SEES ──
        let lines: Vec<&str> = said.lines().collect();
        assert!(
            lines[1].contains(SAID),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 869: the post-promotion check is `grep -E '^run ' -A1`, so a \
             clause anywhere but the line after the heading is one the person doing that check \
             never sees. This is `waiting`'s placement argument on the row this item is about, and \
             a clause that merely appears somewhere in the block would satisfy arm ① alone. \
             Line 2 was: {:?}",
            lines[1],
        );

        // ── ③ THE CONTROL: a pane nobody revived says nothing ──
        //
        // ⚠⚠ Without this the gate passes on a build that prints the clause unconditionally, which
        // would put a restore warning on every healthy loop in the daemon — and a warning that is
        // always there is one nobody reads on the round it is true.
        let fresh = render_run(&restored_run(None));
        assert!(
            !fresh.contains("came back from a restore"),
            "⚠⚠⚠⚠⚠ presence is the claim: a run whose pane the daemon did not revive must carry \
             no such clause, or the four healthy loops on this daemon each get a warning about a \
             restore that did not happen. Got: {fresh}",
        );

        // ── ④ AND A RUN THAT HAS STOPPED SAYS NOTHING, on `walk_to`'s argument one gate over ──
        let finished = render_run(&run_entry(&a_run_that_closed(None)));
        assert!(
            !finished.contains("came back from a restore"),
            "⚠⚠ a finished run's pane is history rather than a question: {finished}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **A RUN SAYS HOW MANY CHECKPOINTS IT SET ASIDE AT ITS DEPTH CAP, WHILE IT IS
    /// STILL RUNNING AND ONCE IT IS OVER** — the owner's decision of 2026-09-02, register item
    /// 833(2), done-when ⑷.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the cap is not finished until this row says so
    ///
    /// Bounding how far a run may re-aim itself is what stops a loop paying its own debt for ever.
    /// The item names its own danger in the same breath: **a cap without this number is
    /// indistinguishable from a loop that never found anything.** A run that deferred eight
    /// proposals and a run whose agent had no ideas publish the same row, and the second is fine
    /// while the first is a repository quietly losing findings.
    ///
    /// So the count has to reach the mouth a PERSON reads, not only the wire. That is the failure
    /// this file has now written down five times — *a fact that reaches the wire and dies at the
    /// mouth somebody actually reads* — and it is why the assertions below drive `render_run` and
    /// not `render_deferred`.
    ///
    /// # ⚠⚠ Four readings, and three of them are the ones a shortcut gets wrong
    ///
    /// * a LIVE run that has deferred — the reading that matters most, because the person seeing it
    ///   can still widen the brief;
    /// * an ENDED run that deferred — the post-mortem, on `render_answered`'s argument that a tally
    ///   present only on convergence is missing from the endings that need explaining;
    /// * a run that deferred NOTHING — `0` is a real claim and prints no clause, or the line
    ///   becomes noise on every healthy row;
    /// * a plugin with no such choice — the key is ABSENT, which must read the same as zero here
    ///   and for a different reason.
    #[test]
    fn a_run_says_how_many_checkpoints_it_set_aside_and_why() {
        /// A live row as the wire really carries one — `deferred` present only when the plugin
        /// answered, which is `progress_to_json`'s own rule.
        fn running(deferred: Option<u64>) -> Value {
            let mut run = serde_json::json!({
                "id": 833,
                "label": "ai_loop pane=2",
                "state": {
                    "status": "running",
                    "iterations": 12,
                    "cost": 0,
                    "unit": "steps",
                    sprag_host::plugins::RUN_ANSWERED_KEY: 0,
                },
            });
            if let Some(set_aside) = deferred {
                run["state"][sprag_host::plugins::RUN_DEFERRED_KEY] = serde_json::json!(set_aside);
            }
            run
        }

        /// The same fact on an ending.
        fn ended(deferred: Option<u64>) -> Value {
            let mut run = serde_json::json!({
                "id": 833,
                "label": "ai_loop pane=2",
                "state": {
                    "status": "done",
                    "outcome": {
                        "state": "converged",
                        "iterations": 40,
                        "cost": 0,
                        "unit": "steps",
                        sprag_host::plugins::RUN_ANSWERED_KEY: 0,
                    },
                },
            });
            if let Some(set_aside) = deferred {
                run["state"]["outcome"][sprag_host::plugins::RUN_DEFERRED_KEY] =
                    serde_json::json!(set_aside);
            }
            run
        }

        // ── THE HEADLINE: a live run that has set two aside says so, and says how many ──
        let live = render_run(&running(Some(2)));
        assert!(
            live.contains("set 2 next checkpoints aside"),
            "🎯🎯🎯🎯🎯 REGISTER ITEM 833(2), done-when ⑷: this run's agent proposed two next \
             checkpoints and the run counted them instead of taking them, and the row a person \
             reads says nothing about it. That row is then identical to one whose agent never had \
             an idea — which is the item's own stated danger, and it turns a depth cap into a way \
             of losing findings quietly. Got: {live}",
        );
        assert!(
            live.contains("registers such things"),
            "⚠⚠⚠ AND THE ROW SAYS WHAT TO DO ABOUT IT. A bare number tells a person something \
             happened and not where to look — and the whole value of the cap is that what it set \
             aside got REGISTERED rather than paid. Got: {live}",
        );

        // ── AND ON THE ENDING, for `render_answered`'s reason ──
        let over = render_run(&ended(Some(3)));
        assert!(
            over.contains("set 3 next checkpoints aside"),
            "⚠⚠⚠⚠ A RUN THAT DEFERRED THREE PROPOSALS AND THEN ENDED is exactly the run somebody \
             reads an outcome to understand — *did it go where I pointed it, and what did it leave \
             behind?* A tally present only while running answers after the reader has gone. Got: \
             {over}",
        );

        // ── ⚠ ONE IS SINGULAR, because a row that says `1 next checkpoints` is a row nobody
        //    proof-read, and this clause is read by a person under time pressure ──
        let single = render_run(&running(Some(1)));
        assert!(
            single.contains("set 1 next checkpoint aside") && !single.contains("checkpoints"),
            "⚠⚠ the singular is the one a real capped run reaches first: {single}",
        );

        // ── ⚠⚠⚠ THE CONTROLS: zero is a CLAIM and prints nothing, and so does an absent key ──
        let healthy = render_run(&running(Some(0)));
        assert!(
            !healthy.contains("aside"),
            "⚠⚠⚠ A RUN THAT NEVER HAD TO SPEND ITS BUDGET MUST NOT CARRY THIS LINE. `0` is a real \
             claim — it had the cap and never met it — and a clause on every healthy row trains a \
             reader to skip the line that matters, which is the argument `render_answered`'s own \
             gate makes one clause over. Got: {healthy}",
        );
        let no_such_choice = render_run(&running(None));
        assert!(
            !no_such_choice.contains("aside"),
            "⚠⚠ AND A PLUGIN WITH NO SUCH CHOICE SAYS NOTHING RATHER THAN ZERO. Every bundled \
             plugin but the loop omits the key, and a row that read the absence as a number would \
             be claiming a decision that plugin cannot make: {no_such_choice}",
        );

        // ══ ⛔⛔⛔⛔⛔ AND WHY EACH ONE WAS SET ASIDE — register item 833 ══════════════════════
        //
        // Measured 2026-09-03 against the live loop daemon: run 189 was STILL GOING, had set three
        // aside, every one of them refused by its kind's `successor_check` and not one at the cap
        // — and this clause said *"at its depth cap"* about all three. The document keeps ONE total
        // deliberately and puts the reason in the ending's word (`capped` / `unadmitted`); a
        // running row has no ending word, so the sentence had been supplying the missing half
        // itself and supplying it wrong.
        //
        // ⚠⚠ THE THREE READINGS MUST DIFFER, because the remedies are opposite: a proposal set
        // aside for BUDGET is registered and a later run may take it; one the kind REFUSED will be
        // refused again. A clause that read the same for both sends half its readers to the wrong
        // remedy — which is exactly the argument `DoneReason`'s own two words were split on.
        let why = |deferred: u64, refused: Option<u64>| -> String {
            let mut run = running(Some(deferred));
            if let Some(refused) = refused {
                run["state"][sprag_host::plugins::RUN_UNADMITTED_KEY] = serde_json::json!(refused);
            }
            render_run(&run)
        };
        let all_budget = why(3, Some(0));
        let all_refused = why(3, Some(3));
        let mixed = why(3, Some(1));
        assert!(
            all_refused.contains("REFUSED") && !all_refused.contains("budget"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 833: three proposals REFUSED by this run's kind read as three \
             set aside for budget. Naming a refused one again gets the same answer, and the row \
             sent its reader to go and register something that is already registered: {all_refused}",
        );
        assert!(
            all_budget.contains("re-aiming budget") && !all_budget.contains("REFUSED"),
            "⛔⛔⛔ ...AND THE OTHER WAY. A run that spent its own re-aiming budget has findings a \
             later run may take, and telling its reader they were refused stops them looking: \
             {all_budget}",
        );
        assert!(
            mixed.contains("REFUSED") && mixed.contains("re-aiming budget"),
            "⚠⚠⚠ AND A RUN CAN BE BOTH. One total with two reasons under it is the shape the \
             document chose on purpose, so the row has to carry the split rather than pick a \
             winner: {mixed}",
        );
        // ⚠ THE CONTROL: the three are three DIFFERENT sentences. Without it a clause that printed
        // one fixed line containing every needle above satisfies all three assertions.
        assert!(
            all_budget != all_refused && all_refused != mixed && all_budget != mixed,
            "⛔ two of the three readings are byte-identical, so the split makes no difference to \
             anybody reading:\n  budget: {all_budget}\n  refused: {all_refused}\n  mixed: {mixed}",
        );
        // ⛔⛔ AND AN OLDER DAEMON CLAIMS NO REASON AT ALL. The key is absent from a daemon that
        // predates it and from every restored run, and a row that filled that in would be inventing
        // the half of the fact it does not have — which is the defect, one build earlier.
        let unsaid = why(3, None);
        assert!(
            unsaid.contains("set 3 next checkpoints aside")
                && !unsaid.contains("REFUSED")
                && !unsaid.contains("re-aiming budget"),
            "⚠⚠⚠⚠ A DAEMON THAT DID NOT SAY MUST NOT BE ANSWERED FOR. The count is still the \
             count; the reason is what an older build cannot say, and supplying one is how this \
             clause came to name `at its depth cap` for three refusals: {unsaid}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **A RUN WHOSE KIND NAMED NO CHECKER SAYS SO ON ITS OWN ROW** — the owner's
    /// decision of 2026-09-03, register item 847, done-when ⑴ and ⑵.
    ///
    /// # ⛔⛔⛔⛔⛔ A bound that shipped switched off, and nothing anywhere said so
    ///
    /// The loop's document names the program that decides whether a proposed next checkpoint may be
    /// taken; the TEMPLATE ships that slot empty so a repository gets the machinery before it has a
    /// judgement to put in it. That is right, and it was **silent**: measured 2026-09-03, no gate in
    /// this workspace named the slot and no sentence a person reads mentioned it, so *a run with the
    /// bound switched off and a run with it satisfied published the same row*.
    ///
    /// ⚠⚠ **AND IT IS THIS REPOSITORY'S OWN WORKING RULE, TURNED ON ITSELF**: an escape hatch must
    /// not disable its own gate, and an unclassified thing is a red rather than a pass. The empty
    /// checker is the fourth time that shape has been paid for here, and the first time it was
    /// inside the machine that carries the rule to everybody else.
    ///
    /// # ⚠⚠⚠ The three readings, and the one a shortcut gets wrong
    ///
    /// * a run that re-aimed itself unchecked — the clause, on the LIVE row and on the ending;
    /// * `0` — *every direction it took was checked*, a real claim that prints nothing;
    /// * an ABSENT key — *this plugin does not re-aim itself*, which must read the same as zero and
    ///   for a different reason. **That is the arm the escape hatch hid behind**, because a build
    ///   that never published the key at all would satisfy every assertion about the clause.
    #[test]
    fn a_run_whose_kind_named_no_checker_says_so_on_its_own_row() {
        /// A live row as the wire really carries one — the key present only when the plugin
        /// answered, which is `progress_to_json`'s own rule.
        fn running(unchecked: Option<u64>) -> Value {
            let mut run = serde_json::json!({
                "id": 847,
                "label": "ai_loop pane=2",
                "state": {
                    "status": "running",
                    "iterations": 12,
                    "cost": 0,
                    "unit": "steps",
                    sprag_host::plugins::RUN_ANSWERED_KEY: 0,
                },
            });
            if let Some(count) = unchecked {
                run["state"][sprag_host::plugins::RUN_UNCHECKED_KEY] = serde_json::json!(count);
            }
            run
        }

        /// The same fact on an ending.
        fn ended(unchecked: Option<u64>) -> Value {
            let mut run = serde_json::json!({
                "id": 847,
                "label": "ai_loop pane=2",
                "state": {
                    "status": "done",
                    "outcome": {
                        "state": "converged",
                        "iterations": 40,
                        "cost": 0,
                        "unit": "steps",
                        sprag_host::plugins::RUN_ANSWERED_KEY: 0,
                    },
                },
            });
            if let Some(count) = unchecked {
                run["state"]["outcome"][sprag_host::plugins::RUN_UNCHECKED_KEY] =
                    serde_json::json!(count);
            }
            run
        }

        // ── THE HEADLINE: a live run that re-aimed twice unchecked says so, and says how many ──
        let live = render_run(&running(Some(2)));
        assert!(
            live.contains("changed direction 2 times with nobody checking"),
            "🎯🎯🎯🎯🎯 REGISTER ITEM 847: this run changed direction twice with its `successor_\
             check` empty, and the row a person reads says nothing about it — so it is identical \
             to a row whose every direction was checked. That is the escape hatch this item is: a \
             bound that ships switched off and makes no sound. Got: {live}",
        );
        assert!(
            live.contains("successor_check"),
            "⚠⚠⚠ AND THE ROW NAMES THE SLOT, because a reader who is told a bound is off and not \
             WHICH bound cannot turn it on. Got: {live}",
        );
        assert!(
            live.contains("name one in that kind's own document"),
            "⚠⚠⚠⚠ AND IT SAYS WHERE TO PUT IT — the kind's document, never the template. The whole \
             design is that the machine is copied and the judgement is authored locally, and a row \
             that sent somebody to edit the shared file would undo it. Got: {live}",
        );

        // ── AND ON THE ENDING, on `render_deferred`'s argument ──
        let over = render_run(&ended(Some(3)));
        assert!(
            over.contains("changed direction 3 times with nobody checking"),
            "⚠⚠⚠⚠ A RUN THAT RE-AIMED ITSELF THREE TIMES ON NOBODY'S SAY-SO AND THEN ENDED is \
             exactly the run whose account must not read like a bounded one. A tally present only \
             while running answers after the reader has gone. Got: {over}",
        );

        // ── ⚠ ONE IS SINGULAR, because a row nobody proof-read is read under time pressure ──
        let single = render_run(&running(Some(1)));
        assert!(
            single.contains("changed direction ONCE with nobody checking")
                && !single.contains("times with nobody"),
            "⚠⚠ the singular is the one a real run reaches first: {single}",
        );

        // ── ⚠⚠⚠ THE CONTROLS: zero is a CLAIM and prints nothing, and so does an absent key ──
        let checked = render_run(&running(Some(0)));
        assert!(
            !checked.contains("nobody checking"),
            "⚠⚠⚠ A RUN WHOSE EVERY DIRECTION WAS CHECKED MUST NOT CARRY THIS LINE. `0` is a real \
             claim, and a clause on every healthy row trains a reader to skip the line that \
             matters. Got: {checked}",
        );
        let no_such_choice = render_run(&running(None));
        assert!(
            !no_such_choice.contains("nobody checking"),
            "⚠⚠ AND A PLUGIN THAT DOES NOT RE-AIM ITSELF SAYS NOTHING RATHER THAN ZERO. Every \
             bundled plugin but the loop omits the key, and a row reading that absence as a number \
             would accuse it of a decision it cannot make: {no_such_choice}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN A BOOT PUT BACK SAYS WHETHER IT WENT BACK TO WORK** — register item 774,
    /// done-when ⑴.
    ///
    /// # The measurement this exists for
    ///
    /// One promotion, 2026-08-30: three loops came back and, two hours later, had made **zero
    /// deliveries between them**; four runs started in the same window had made one each. All seven
    /// rows read `running`, and the way anybody found out was comparing log records by hand.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the reading is an ABSENT key rather than a zero
    ///
    /// `run_to_json` publishes the delivery triple **only when one of them is non-zero**, and its
    /// own comment says why: a plugin that composes nothing would otherwise publish three zeroes
    /// that read as *it typed nothing*. So the run this item is about carries no delivery key at
    /// all — there was no number for the row to be silent about, which is the mechanical half the
    /// register could not see. ⚠ And the one thing that absence cannot separate — a plugin that
    /// composes no prompts — is named in the clause rather than left for a reader.
    ///
    /// ⚠⚠ FOUR READINGS, DRIVEN AS VALUES, because a clause written for *the next promotion* is a
    /// clause nobody has run (register item 803): a rescued run mid-work, one that has stepped and
    /// typed nothing, one that has not stepped at all, and a run nobody rescued.
    #[test]
    fn a_rescued_run_says_whether_it_went_back_to_work_and_a_fresh_one_says_nothing() {
        /// A row as the wire really carries one. `resumed` and `delivered` are the two facts under
        /// test; `steps` is what separates *has not started yet* from *started and stayed silent*.
        fn row(resumed: bool, delivered: Option<u64>, steps: u64) -> Value {
            let mut run = serde_json::json!({
                "id": 102,
                "label": "ai_loop pane=369",
                "state": {
                    "status": "running",
                    "iterations": steps,
                    "cost": 0,
                    "unit": "steps",
                },
            });
            if resumed {
                run[sprag_host::plugins::RUN_RESUMED_KEY] = serde_json::json!(true);
            }
            if let Some(made) = delivered {
                run[sprag_host::plugins::RUN_DELIVERED_KEY] = serde_json::json!(made);
            }
            run
        }

        // ── THE HEADLINE: rescued, stepping, and nothing typed ──
        let silent = render_run(&row(true, None, 5));
        assert!(
            silent.contains("put back by a boot") && silent.contains("5 step(s)"),
            "⛔⛔⛔⛔⛔ ITEM 774: this run came back, took five steps and delivered nothing, and its \
             row says `running` exactly as a run started a second ago does. That is the state three \
             loops sat in for two hours while a person compared log records by hand. Got: {silent}",
        );
        assert!(
            silent.contains("composes no prompts"),
            "⚠⚠⚠ AND THE READING IT CANNOT MAKE IS SAID: an absent delivery key is *nothing was \
             typed* OR *this plugin composes nothing*, and a clause that claimed the first alone \
             would send somebody to a pane about a `pipe` run: {silent}",
        );

        // ⚠⚠ ON THE STATUS LINE, which is `waiting`'s placement and for its measured reason: this
        // repository's outer-loop watcher reads the line after the heading and nothing else, so a
        // detail line would be invisible to the one reader that was actually watching.
        let lines: Vec<&str> = silent.lines().collect();
        assert!(
            lines[1].contains("put back by a boot"),
            "⚠⚠⚠ the clause must land on the status line — a watcher that reads `$0 ~ r {{getline; \
             print}}` sees that line and no other: {silent}",
        );

        // ── ⚠ A RESCUED RUN THAT HAS NOT STEPPED YET IS A DIFFERENT ANSWER, not the same warning.
        //    The boot has just handed it over; sending somebody to a pane now finds nothing.
        let just_back = render_run(&row(true, None, 0));
        assert!(
            just_back.contains("has not taken a step yet") && !just_back.contains("step(s)"),
            "⚠⚠ a run handed back a moment ago has typed nothing for a reason nobody should act \
             on, and folding it in with the silent one would train a reader to ignore both: \
             {just_back}",
        );

        // ── ⚠⚠ THE CONTROL, AND IT IS THE HALF THAT KEEPS THIS CLAUSE FROM BEING NOISE: a rescued
        //    run that IS delivering says nothing at all.
        let working = render_run(&row(true, Some(3), 9));
        assert!(
            !working.contains("put back by a boot"),
            "⚠⚠⚠⚠ a rescued run that is plainly working must be silent here, or the clause appears \
             on every resumed row for ever and stops meaning anything — which is the failure mode \
             `render_answered` and `walk_to` are each shaped around: {working}",
        );

        // ── ⚠⚠⚠ AND THE SECOND CONTROL: a run NOBODY rescued must never claim it was. The whole
        //    clause rests on a fact only the registry can answer, and a renderer that inferred it
        //    from *no deliveries yet* would print it over every run's first seconds.
        let fresh = render_run(&row(false, None, 5));
        assert!(
            !fresh.contains("put back by a boot"),
            "⛔⛔⛔ a run this daemon started is not a run a boot rescued, and a row that said so \
             would point a reader at a restart that never happened: {fresh}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RESCUED RUN WHOSE NUMBERS ARE STILL A DEAD DAEMON'S SAYS SO** — register item
    /// 815, and the window the gate above structurally could not see.
    ///
    /// # The measurement
    ///
    /// 2026-09-01, on the live fixture item 774's own gate stages: a run a boot had put back onto a
    /// pane that came back a plain shell published `running — 2 iterations` and `1 prompt(s)
    /// delivered, all of them on that pane` — while its new driver had taken **no step** and typed
    /// **nothing**. Both numbers were read out of the predecessor's log by `RunRegistry::restore`,
    /// which is deliberate (items 606 and 616); what was missing is the word for whose they are.
    ///
    /// ⚠⚠⚠⚠ **AND THE COST IS ITEM 774's CLAUSE GOING SILENT ON ITS OWN CASE.** That clause reads
    /// an ABSENT delivery count as *nothing has been typed since*, so a restored count is not a
    /// stale number beside a working warning — it is the thing that switches the warning off.
    ///
    /// ⚠⚠ FOUR READINGS DRIVEN AS VALUES (register item 803): inherited with a count, inherited
    /// without one, a rescued run whose driver HAS spoken, and a restored run nobody put back.
    #[test]
    fn a_rescued_run_whose_counters_are_its_predecessors_says_so_before_it_reads_them() {
        /// A row as the wire carries one. `inherited` is the fact under test; `delivered` is what
        /// used to silence the clause.
        fn row(resumed: bool, inherited: bool, delivered: Option<u64>, steps: u64) -> Value {
            let mut run = serde_json::json!({
                "id": 104,
                "label": "ai_loop pane=1",
                "state": {
                    "status": "running",
                    "iterations": steps,
                    "cost": 0,
                    "unit": "steps",
                },
            });
            if resumed {
                run[sprag_host::plugins::RUN_RESUMED_KEY] = serde_json::json!(true);
            }
            if inherited {
                run[sprag_host::plugins::RUN_INHERITED_KEY] = serde_json::json!(true);
            }
            if let Some(made) = delivered {
                run[sprag_host::plugins::RUN_DELIVERED_KEY] = serde_json::json!(made);
            }
            run
        }

        // ── THE HEADLINE: rescued, nothing has driven it since, and a stale count on the row ──
        let stale = render_run(&row(true, true, Some(1), 2));
        assert!(
            stale.contains("nothing has driven it since") && stale.contains("the predecessor's"),
            "⛔⛔⛔⛔⛔ ITEM 815: this run was put back by a boot, no driver of this daemon has \
             said a word, and the row publishes a delivery count and a step count that a dead \
             process earned. A person reads `running` and `1 prompt(s) delivered` and concludes \
             the loop is working — which is exactly the state item 774 was filed over, wearing \
             that item's own repair as a disguise. Got: {stale}",
        );
        // ⚠⚠ AND IT IS ON THE STATUS LINE, `waiting`'s and item 774's placement, for their measured
        // reason: this repository's outer-loop watcher reads the line after the heading and no
        // other, and a clause it cannot see repairs nothing for the reader that was watching.
        let lines: Vec<&str> = stale.lines().collect();
        assert!(
            lines[1].contains("nothing has driven it since"),
            "⚠⚠⚠ the clause must land on the status line — a watcher that reads `$0 ~ r {{getline; \
             print}}` sees that line and no other: {stale}",
        );

        // ── ⚠ THE SAME ROW WITHOUT A COUNT IS THE SAME ANSWER, not item 774's step sentence: a
        //    step count that is also the predecessor's cannot say *it has stepped and stayed
        //    silent*, which is what that sentence claims.
        let countless = render_run(&row(true, true, None, 2));
        assert!(
            countless.contains("nothing has driven it since")
                && !countless.contains("step(s) with no delivery"),
            "⚠⚠⚠ ITEM 815: a rescued row whose numbers are its predecessor's was given item 774's \
             reading, which asserts this driver has taken steps. It has taken none — the count is \
             the dead daemon's — and a reader told otherwise goes to a pane looking for work that \
             was never started here: {countless}",
        );

        // ── ⚠⚠ THE CONTROL THAT KEEPS THIS FROM BEING NOISE: a rescued run whose driver HAS
        //    spoken says none of it, and item 774's own readings take over again.
        let spoken = render_run(&row(true, false, Some(3), 9));
        assert!(
            !spoken.contains("nothing has driven it since"),
            "⚠⚠⚠⚠ a rescued run whose own driver reported must not be told its numbers are \
             somebody else's, or the clause appears on every resumed row for ever and stops \
             meaning anything: {spoken}",
        );

        // ── ⚠⚠⚠ AND THE SECOND CONTROL: a restored row NOBODY put back says nothing here. The
        //    numbers are a predecessor's for that row too, and the sentence is about a run a boot
        //    handed to a driver — `interrupted` rows are item 771's and item 737's to explain.
        let untouched = render_run(&row(false, true, Some(1), 2));
        assert!(
            !untouched.contains("nothing has driven it since"),
            "⛔⛔ ITEM 815: a run no boot put back is being told a driver failed to speak for it. \
             Nothing was ever going to: that row is `interrupted` and its own clauses say why. \
             This sentence is about a rescue that went quiet: {untouched}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **`sprag words` ANSWERS WITHOUT A DAEMON, AND REFUSES A NAME BY NAMING WHAT IT
    /// HAS** — register item 773, and the two properties that make it a replacement for a table.
    ///
    /// # Why *without a daemon* is asserted rather than assumed
    ///
    /// It is the whole reason this is a verb here instead of a `scene/query`. The words are
    /// compiled in, so the one moment a person most needs them — a run has ended, the daemon is
    /// down, and a row says `taken_over` — is exactly when a daemon-served answer would be silent.
    /// This test runs in a process with no socket set and no daemon anywhere, which is the same
    /// condition, and it is asserted by CALLING the verb rather than by reading its code.
    ///
    /// ⚠⚠ AND THE REFUSAL IS HELD TO NAMING EVERY VOCABULARY. A message that named some of them
    /// sends a reader back to the source tree, which is the thing item 773 measured aging.
    #[test]
    fn asking_this_build_for_its_words_needs_no_daemon_and_an_unknown_name_says_what_there_is() {
        words(Vec::new()).expect(
            "⛔⛔⛔⛔⛔ ITEM 773: `sprag words` could not answer with no daemon running. A closed \
             vocabulary is compiled into this binary, and a verb that needed a socket to say so \
             would be silent at the one moment a person is reading a finished run.",
        );
        for (name, _, _) in sprag_host::plugins::RUN_VOCABULARIES {
            words(vec![(*name).to_owned()])
                .unwrap_or_else(|why| panic!("`sprag words {name}` answers: {why}"));
        }

        let refused = words(vec!["outcomes".to_owned()])
            .expect_err("⚠⚠ THE PREMISE: a name no vocabulary has must be refused, not printed");
        let said = refused.to_string();
        assert!(
            said.contains("outcomes"),
            "⚠⚠⚠ the refusal must quote what was ASKED, or a person with a typo cannot see it: \
             {said}",
        );
        for (name, _, _) in sprag_host::plugins::RUN_VOCABULARIES {
            assert!(
                said.contains(name),
                "⛔⛔⛔ ITEM 773: this refusal does not name `{name}`, so a caller who mistyped is \
                 told what they asked for is wrong and left to go and find the list — in the \
                 source tree this verb exists to stop them needing. Said: {said}",
            );
        }

        let extra = words(vec!["verdict".to_owned(), "outcome".to_owned()])
            .expect_err("⚠⚠ two names at once is refused rather than half-honoured");
        assert!(
            extra.to_string().contains("one at a time"),
            "⚠ and the refusal says what the shape is: {extra}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY ENDING THIS BUILD CAN RECORD IS PUBLISHED WITH WHAT HAPPENS NEXT TO IT, AND
    /// THE FOUR ANSWERS ARE FOUR DIFFERENT ANSWERS** — register item 867.
    ///
    /// # What this holds that the type's own gates do not
    ///
    /// `sprag_plugin` holds the classification: every outcome has a disposition, and a seventh
    /// outcome joins [`OutcomeState::EVERY_SHAPE`] or nothing compiles. What nothing held before
    /// this is the MOUTH — item 827 put those sentences on a run's row and left every reader who is
    /// not asking about one particular run with no way to ask at all, which is the whole of item
    /// 867's measurement (a push-time reader holding six words and nowhere to send them).
    ///
    /// # ⚠⚠ The expectation is DERIVED, never spelled
    ///
    /// Each row is checked against [`Disposition::wire_str`] and [`Disposition::describe`] read
    /// from the type, so this cannot go green on a renderer that has drifted from the
    /// classification — the defect items 855 and 864 each paid for. What it CAN catch is the
    /// renderer, which is the thing it is for.
    ///
    /// ⚠ **AND THAT THE ANSWERS DIFFER.** A build whose four dispositions rendered to one sentence
    /// would satisfy every per-row assertion and publish a classification that makes no difference,
    /// which is the state item 827 was filed on wearing a table.
    #[test]
    fn what_happens_next_is_published_for_every_ending_and_refused_for_the_rest() {
        use sprag_plugin::driver::Disposition;

        disposition(Vec::new()).expect(
            "⛔⛔⛔⛔⛔ ITEM 867: `sprag disposition` could not answer with no daemon running. The \
             classification is compiled into this binary, and the reader it exists for — \
             `.githooks/loop-read.sh` — runs at push time with no daemon at all.",
        );

        let rows = disposition_rows();
        assert_eq!(
            rows.len(),
            Disposition::table().count(),
            "one row per ending, and the count comes from the type: {rows:?}",
        );
        for (word, next) in Disposition::table() {
            let row = rows
                .iter()
                .find(|row| row.split_whitespace().next() == Some(word))
                .unwrap_or_else(|| {
                    panic!(
                        "⛔⛔⛔ ITEM 867: no row's FIRST FIELD is `{word}`. That field is the \
                         column `.githooks/loop-read.sh` matches a run's ending on, so a row that \
                         leads with anything else is a push that says nothing about that \
                         ending.\n  rows: {rows:?}"
                    )
                });
            assert!(
                row.contains(next.wire_str()) && row.contains(next.describe()),
                "⛔⛔ ITEM 867: `{word}`'s row does not carry what the type says happens next \
                 (`{}`). A row naming an ending without saying what follows it moves the lookup \
                 back into the reader's head, which is where the answer already was.\n  row: \
                 {row}\n  wanted: {}",
                next.wire_str(),
                next.describe(),
            );
            // ⛔⛔⛔⛔⛔ AND THE PARTY THAT IS OWED THE NEXT RUN — register item 872(1). The
            // permission column says a machine MAY; this says WHO IS OWED, and item 872 measured
            // endings that had the first and got no next run because nothing held the second. A
            // row that drops this column publishes a permission with nobody attached to it.
            assert!(
                row.contains(next.opens_next().wire_str())
                    && row.contains(next.opens_next().describe()),
                "⛔⛔ ITEM 872(1): `{word}`'s row does not name what opens the next run (`{}`). \
                 Without it a reader can see that an ending permitted a next run and still cannot \
                 say who failed to open one.\n  row: {row}\n  wanted: {}",
                next.opens_next().wire_str(),
                next.opens_next().describe(),
            );
            disposition(vec![word.to_owned()])
                .unwrap_or_else(|why| panic!("`sprag disposition {word}` answers: {why}"));
        }
        // ⚠⚠ THE CONTROL: four dispositions, four DISTINCT sentences in the table. Without this a
        // build that rendered one answer for everything passes every assertion above.
        let mut sentences: Vec<&str> = Disposition::ALL
            .iter()
            .map(|next| next.describe())
            .collect();
        sentences.sort_unstable();
        sentences.dedup();
        assert_eq!(
            sentences.len(),
            Disposition::ALL.len(),
            "⛔ two dispositions say the same thing, so the split this table publishes makes no \
             difference to anybody reading it",
        );

        // ⛔ AN UNCLASSIFIED ENDING IS A REFUSAL THAT NAMES WHAT THERE IS — rule 6. `panicked` is a
        // real word of this product (a `run_status`), which is exactly the kind of thing a caller
        // arrives holding.
        let refused = disposition(vec!["panicked".to_owned()]).expect_err(
            "⚠⚠ THE PREMISE: an ending nothing classifies must be refused, never answered — a \
             caller told nothing reads it as *nothing to do*, which is the answer item 827 says \
             must be recorded rather than invented",
        );
        let said = refused.to_string();
        assert!(
            said.contains("panicked"),
            "⚠⚠⚠ the refusal must quote what was ASKED: {said}",
        );
        for (word, _) in Disposition::table() {
            assert!(
                said.contains(word),
                "⛔⛔⛔ ITEM 867: this refusal does not name `{word}`, so a caller is told their \
                 word is wrong and left to go and find the list. Said: {said}",
            );
        }

        let extra = disposition(vec!["failed".to_owned(), "converged".to_owned()])
            .expect_err("⚠⚠ two endings at once is refused rather than half-honoured");
        assert!(
            extra.to_string().contains("one at a time"),
            "⚠ and the refusal says what the shape is: {extra}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE MOUTH SAYS BOTH HALVES, AND REFUSES RATHER THAN PRINTING AN EMPTY TABLE** —
    /// register item 872 ⑶, the crossing half.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the mouth needs a gate of its own
    ///
    /// `RunLog::waits_between_runs` is gated where it lives, over a log with every arm reached. Item
    /// 856 ⑸ measured what that is not enough for: seven surfaces a value passes THROUGH, each
    /// gated, and the call that puts it in replaced by a discard left the whole workspace green.
    /// The renderer is that call here — it is where the unmeasurable half can quietly stop being
    /// printed, which turns *229 runs behind a promotion wall* into a blank page.
    ///
    /// # ⛔ AND NO LOG READ IS A REFUSAL, NOT AN EMPTY ANSWER
    ///
    /// This workspace's rule 6. *Nothing was read* and *nothing waited* are opposite facts, and a
    /// verb that printed a heading and stopped would say the second while meaning the first — to a
    /// reader whose whole question is whether a delay is still there.
    #[test]
    fn what_a_tree_waited_is_printed_beside_what_could_not_be_measured() {
        use sprag_host::runs::{LeftEnd, NoWait, RunLog};

        // A log holding one measurable stretch on one tree, one row from before item 890's column,
        // and one row a dead daemon walked away from — the halves this verb exists to print
        // together. ⚠ Run 0 is FIRST and carries its own build word: what makes it abandoned is
        // that a LATER row names a different build, so its position in the log is the evidence.
        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": sprag_host::runs::RUN_LOG_VERSION,
            "runs": [
                {"id": 0, "label": "ai_loop pane=0", "iterations": 1, "finished": false,
                 "place": ["working"], "tree": "/w", "ran_from": 1, "build": "older"},
                {"id": 1, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                 "place": ["working"], "tree": "/w", "ran_from": 10, "ran_to": 100,
                 "build": "newer"},
                {"id": 2, "label": "ai_loop pane=2", "iterations": 1, "finished": true,
                 "place": ["working"], "tree": "/w", "ran_from": 13729, "ran_to": 13800,
                 "build": "newer"},
                {"id": 3, "label": "ai_loop pane=3", "iterations": 1, "finished": true,
                 "place": ["working"], "build": "newer"},
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");
        let here = std::path::PathBuf::from("/tmp/one.runs.json");
        let lines = waits_lines(&[(here.clone(), log.clone())], None);
        let said = lines.join("\n");

        // ── ① THE STRETCH, WITH ITS NUMBER — item 827's own shape, 13,629s being 3h47m ──
        assert!(
            said.contains("/w waited 13629s after run 1 until run 2"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 872 ⑶: the verb computed the delay and did not print it. Item \
             827 measured 3 h 49 m by hand and four rounds recorded the clause as unmeasurable; a \
             renderer that drops the number puts it back where it was. Got:\n{said}",
        );

        // ── ② AND WHAT IT COULD NOT MEASURE, WHICH IS THE HALF THAT GOES QUIET ──
        assert!(
            said.contains(NoWait::TreeUnknown.describe())
                && said.contains(NoWait::NothingFollowed.describe()),
            "⛔⛔⛔⛔⛔ THE UNMEASURABLE HALF IS NOT ON THE PAGE. Today's real store is 229 rows of \
             exactly that (2026-09-05T07:53:28Z), so a verb printing only its stretches answers a \
             blank page for the machine it was built for — and a blank page reads as *no tree ever \
             waited*, which is the strongest claim there is made from no evidence. Got:\n{said}",
        );

        // ── ③ THE CONTROL: an arm nothing reached is NOT printed ──
        //
        // ⚠⚠ Without this, a renderer listing all six arms every time passes ② by saying
        // everything — and a report where every line is always present is one nobody reads on the
        // round it matters. Item 856's `0 of 0` rule.
        assert!(
            !said.contains(NoWait::SuccessorStartedFirst.describe()),
            "⚠⚠ an arm no run reached must not appear: a table that always prints six lines has \
             stopped distinguishing them. Got:\n{said}",
        );

        // ── ③b AND THE SECOND AXIS REACHES THE PAGE — register item 872 ⑶b ──
        //
        // ⛔⛔⛔⛔⛔ Run 3 above is the row today's whole store is made of: no tree, so the first
        // axis says `TreeUnknown` and stops. It is ALSO a run nobody watched stop, and that is the
        // fact deciding whether the number item 872 ⑶ wants can ever come out of such rows. It
        // cannot. Measured over the live store 2026-09-05T12:11:19Z: 231 rows, `TreeUnknown` 231,
        // watched stops **0** — so *backfill item 890's column* is not a route to the number, and
        // a page carrying only the first axis reads as though it were.
        assert!(
            said.contains("2 of 4 run(s) carry the watched stop a stretch is measured from")
                && said.contains(LeftEnd::Unwatched.describe()),
            "⛔⛔⛔⛔⛔ THE SECOND AXIS IS NOT ON THE PAGE. `NoWait` is tried in order and stops at \
             `TreeUnknown`, which is every row of the real store, so without this line the report \
             names one wall where there are two and the reader is invited to fill in a column that \
             would change nothing. Got:\n{said}",
        );
        // ── ③c AND THE WALL THAT WILL NEVER LIFT IS TOLD APART FROM THE ONE THAT MIGHT ──
        //
        // ⛔⛔⛔⛔⛔ Run 0 has not finished and never will: a later row of this log names a
        // different build, so the daemon that would have watched it stop is gone. Read against the
        // live store at 2026-09-05T12:35:11Z that is 19 rows of 21 — twelve promotions' worth,
        // back to run 15 — and the page used to call all 21 *have not ended*, which invites the
        // one reading that is false: come back later.
        assert!(
            said.contains(LeftEnd::Abandoned.describe())
                && said.contains(NoWait::EndAbandoned.describe()),
            "⛔⛔⛔⛔⛔ A ROW NOTHING WILL EVER WATCH STOP WAS REPORTED AS ONE THAT HAS NOT ENDED \
             YET, on one axis or both. `LeftEnd::NotEndedYet` can become `Watched` and \
             `Abandoned` cannot, so merging them puts a role in the population that no waiting \
             empties — this workspace's rule 5, printed. Got:\n{said}",
        );
        assert!(
            !said.contains(LeftEnd::NotEndedYet.describe()),
            "⚠⚠ and its empty arms drop at the mouth like the first axis's — item 856's `0 of 0` \
             rule, held on both axes rather than on whichever was written first. ⚠⚠ It is ALSO the \
             control for ③c: with `Abandoned` folded back into `NotEndedYet` this line reddens, so \
             the two arms cannot pass for each other in either direction. Got:\n{said}",
        );

        // ── ④ AND NOTHING READ IS A REFUSAL ──
        let refused = waits(vec!["/nonexistent/nothing.runs.json".to_owned()])
            .expect_err("⛔ a log that cannot be read is a REFUSAL, never an empty table");
        assert_eq!(refused.kind(), io::ErrorKind::NotFound);
        assert!(
            refused.to_string().contains("NOTHING WAS READ"),
            "⚠⚠ and it says which of the two silences this is, because *nothing was read* and \
             *nothing waited* are opposite facts that print alike: {refused}",
        );

        let extra = waits(vec!["a".to_owned(), "b".to_owned()])
            .expect_err("⚠⚠ two logs at once is refused rather than half-honoured");
        assert!(
            extra.to_string().contains("one at a time"),
            "⚠ and the refusal says what the shape is: {extra}",
        );

        // ── ⑤ AND A SWEPT ANSWER NAMES THE DIRECTORY IT IS ABOUT ──
        //
        // ⛔⛔⛔⛔⛔ MEASURED, not imagined. The loop exports its own `XDG_STATE_HOME` and a
        // push-time hook inherits none, so `sprag waits` reads a DIFFERENT directory depending on
        // who runs it — 2026-09-05T08:47:23Z, the same command on the same machine answered about
        // the loop's 229 runs with the variable and about **62 integration-test runs across twelve
        // logs** without it. Both tables are long, confident and complete. Nothing in the output
        // said which one you were looking at.
        //
        // ⚠⚠ `.githooks/loop-read.sh` holds the same hazard as a named sentence for the same
        // directory; there the wrong place prints ZEROS, which reads as *nothing happened*. Here it
        // prints a full table, which reads as *this is your machine* — so the address has to be on
        // the page.
        let swept = waits_lines(&[(here, log)], Some(std::path::Path::new("/state/sprag")));
        assert!(
            swept.first().is_some_and(
                |line| line.contains("/state/sprag") && line.contains("XDG_STATE_HOME")
            ),
            "⛔⛔⛔⛔⛔ A SWEPT TABLE DID NOT SAY WHICH DIRECTORY IT SWEPT. The path is derived \
             from the environment and the fallback is a real directory full of integration-test \
             logs, so an answer that does not name its address cannot be doubted by the person \
             holding it. Got: {swept:?}",
        );
        assert!(
            !said.contains("XDG_STATE_HOME"),
            "⚠⚠ AND A NAMED LOG CARRIES NO SUCH WARNING: the caller said where, so a note about a \
             derivation that did not happen would make the one certain case read as the doubtful \
             one. Got:\n{said}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE MOUTH KEEPS THE TWO LANDING COUNTS APART, AND WITHHOLDS THEM OVER AN EMPTY
    /// POPULATION** — register item 856 ⑴, the crossing half.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the mouth needs a gate of its own
    ///
    /// `RunLog::folds_against_fullness` is gated where it lives. Item 856 ⑸ measured what that is
    /// not enough for: seven surfaces a value passes through, each gated, and the call that puts it
    /// in replaced by a discard left the whole workspace green. The renderer is that call here, and
    /// it is where two numbers that must never be added could quietly become a sum.
    ///
    /// # ⛔⛔⛔ AND A ZERO OVER NO POPULATION IS WITHHELD RATHER THAN PRINTED
    ///
    /// The first run of this verb at the real store printed `0 capacity landing(s) refute the axis`
    /// above a store where **nothing at all is readable** (2026-09-05T10:29:27Z, 229 rows). That
    /// reads as *the axis survived*, and a zero being read as clean is this register item's own
    /// most repeated failure. So the counts appear only when some run could be read, and the
    /// population is said out loud when none could.
    #[test]
    fn the_two_landing_counts_print_apart_and_are_withheld_over_no_population() {
        use sprag_host::runs::{NoFullness, RunLog};

        // One run judged by its own document, one whose ceiling a caller moved, and one behind the
        // promotion wall — the three shapes this verb exists to print together.
        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": sprag_host::runs::RUN_LOG_VERSION,
            "runs": [
                {"id": 1, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                 "context_high_water": 800_000, "context_ceiling": 800_000, "overridden": [],
                 "folds_by_reason": {"capacity": {"delivered": 4, "folded": 1},
                                     "ordinary": {"delivered": 40, "folded": 0}}},
                {"id": 2, "label": "ai_loop pane=2", "iterations": 1, "finished": true,
                 "context_high_water": 24_000, "context_ceiling": 20_000,
                 "overridden": ["context_ceiling"],
                 "folds_by_reason": {"capacity": {"delivered": 28, "folded": 1}}},
                {"id": 3, "label": "ai_loop pane=3", "iterations": 1, "finished": true,
                 "folds_by_reason": {"capacity": {"delivered": 3, "folded": 3}}},
                // ⛔⛔⛔⛔⛔ RUN 232's OWN SHAPE, byte for byte in the fields that decide: an
                //    ORDINARY run carrying a fullness BELOW its own document's ceiling, whose
                //    `capacity` road was never taken. This is what the very first row item 856 ⑴⒞
                //    ever produced looked like (2026-09-05T13:01:38Z), and the mouth called it a
                //    recording defect. Nothing in this fixture had a peak below its ceiling, so
                //    the arm was unreachable and only the live store could find it.
                {"id": 4, "label": "ai_loop pane=4", "iterations": 5, "finished": false,
                 "context_high_water": 303_328, "context_ceiling": 800_000, "overridden": [],
                 "folds_by_reason": {"ordinary": {"delivered": 2, "folded": 0}}},
                // ⚠⚠ AND ITS CONTROL, which is what keeps the fix from being *never warn*: the
                //    SAME two columns disagreeing on a run that DID walk the capacity road. There
                //    the document turned on `context >= context_ceiling` and the peak is a peak
                //    over those readings, so a peak below the ceiling really is a recording defect.
                {"id": 5, "label": "ai_loop pane=5", "iterations": 9, "finished": true,
                 "context_high_water": 100_000, "context_ceiling": 800_000, "overridden": [],
                 "folds_by_reason": {"capacity": {"delivered": 1, "folded": 1}}},
                // ⚠⚠ AND THE THIRD WAY THE ROAD IS TAKEN, which is the half a `delivered > 0` test
                //    drops: the reflection happened and the question NEVER GOT ASKED. The
                //    transition is what says the session reached its ceiling; whether a prompt
                //    came out of it is a later fact, and item 856 ⑶ exists because that half was
                //    outside the delivered/folded pair entirely.
                {"id": 6, "label": "ai_loop pane=6", "iterations": 9, "finished": true,
                 "context_high_water": 90_000, "context_ceiling": 800_000, "overridden": [],
                 "folds_by_reason": {"capacity": {"delivered": 0, "folded": 0,
                                                  "unasked_after_a_fold": 1,
                                                  "unasked_on_the_pane": 0}}},
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");
        let here = std::path::PathBuf::from("/tmp/one.runs.json");
        let lines = folds_lines(&[(here.clone(), log.clone())], None);
        let said = lines.join("\n");

        // ── ① THE ROW: a fullness, the ceiling beside it, and whose ceiling it was ──
        assert!(
            said.contains("run 1: 800000 read of a 800000 ceiling, its document's ceiling")
                && said.contains("capacity 1 of 4")
                // ⚠⚠ AND THE OTHER ROAD, which is the CONTROL and the half a renderer would drop
                // first: *long prompts fold* is a live rival explanation for every capacity fold,
                // and only the ordinary traffic beside it tells the two apart.
                && said.contains("ordinary 0 of 40"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴: the left-hand term, the right-hand term and the WHOLE \
             split have to arrive on ONE line or a reader is comparing two tables by eye — which \
             is what five re-judgements of this item did with a `python3 -c`. Got:\n{said}",
        );

        // ── ② THE TWO LANDING COUNTS, PRESENT AND NOT POOLED ──
        assert!(
            said.contains("3 capacity landing(s) refute the axis")
                && said.contains("27 further landing(s) are at a ceiling a caller moved"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴: 3 landings under the document's own ceiling refute the \
             axis and 27 under a ceiling a caller MOVED do not — at a moved ceiling the reflection \
             means *we handed over early*. The live store held 29 of the second kind and a round \
             quoted them as the first for a day. A mouth printing one number instead of two puts \
             that back. Got:\n{said}",
        );
        assert!(
            !said.contains("30 capacity landing(s)"),
            "⛔⛔⛔ AND NEVER THEIR SUM, which is the same sentence as ② said the way it actually \
             went wrong. Got:\n{said}",
        );

        // ── ②b THE CAVEAT FIRES ON A RECORDING DEFECT AND NOT ON A HEALTHY SESSION ──
        //
        // ⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴⒞. Run 4 is the shape of the first ordinary row this item
        // ever got a fullness from, and the mouth printed *the two columns disagree* over it
        // (2026-09-05T13:01:55Z). A run that has not reflected on capacity is SUPPOSED to sit below
        // its ceiling — that is every healthy run, every moment before its first reflection — so
        // the warning was on the one row ⒞'s whole number rests on. Run 5 is the control: it DID
        // walk that road, so there the disagreement is real.
        let caveat = " ⚠ ITS PEAK IS BELOW THAT CEILING, so the two columns disagree";
        let row_of = |id: u32| {
            lines
                .iter()
                .find(|line| line.trim_start().starts_with(&format!("run {id}:")))
                .unwrap_or_else(|| panic!("run {id} must have a row of its own in:\n{said}"))
                .clone()
        };
        assert!(
            !row_of(4).contains(caveat),
            "⛔⛔⛔⛔⛔ A HEALTHY SESSION WAS CALLED A RECORDING DEFECT. Run 4 never took the \
             `capacity` road, so its peak is below its ceiling for the only reason an unfinished \
             run's peak ever is: it has not filled up. `reached_its_ceiling` only promises to \
             agree WHERE THAT ROAD WAS TAKEN — its own doc says so — and the mouth read the answer \
             without the condition. Got:\n{}",
            row_of(4),
        );
        assert!(
            row_of(5).contains(caveat),
            "⛔⛔⛔ AND THE CONTROL, or the fix is *never warn*: run 5 walked the capacity road, \
             where the document turns on `context >= context_ceiling` and the peak is taken over \
             those readings — so a peak below the ceiling there is a defect in the recording and \
             has to be said. Got:\n{}",
            row_of(5),
        );
        assert!(
            row_of(6).contains(caveat),
            "⚠⚠ AND THE ROAD IS TAKEN BY THE UNASKED HALF TOO. Run 6's capacity reflection never \
             produced a question, so `delivered` is 0 — but the document still transitioned on \
             `context >= context_ceiling`, which is the whole reason the peak is promised to \
             agree. A condition written as `delivered > 0` silences the warning on exactly the \
             runs item 856 ⑶ was opened for. Got:\n{}",
            row_of(6),
        );

        // ── ③ AND WHAT COULD NOT BE READ, WHICH IS THE HALF THAT GOES QUIET ──
        assert!(
            said.contains(NoFullness::FullnessUnread.describe()),
            "⛔⛔⛔⛔⛔ THE UNREADABLE HALF IS NOT ON THE PAGE. Today's real store is 229 rows of \
             exactly that (2026-09-05T10:30:15Z), so a verb printing only its rows answers a blank \
             page for the machine it was built for. Got:\n{said}",
        );
        // ⚠ THE CONTROL: an arm nothing reached is not printed — item 856's own `0 of 0` rule. A
        // renderer listing all five arms every time satisfies ③ by saying everything.
        assert!(
            !said.contains(NoFullness::CeilingUnrecorded.describe()),
            "⚠⚠ an arm no run reached must not appear: a table that always prints five lines has \
             stopped distinguishing them. Got:\n{said}",
        );

        // ── ④ AND OVER A STORE WHERE NOTHING IS READABLE, THE COUNTS ARE WITHHELD ──
        //
        // ⛔ This is today's real store, and the reading that made this arm: `0 landing(s) refute
        // the axis` printed above 229 unreadable rows is a zero that reads as *the axis survived*.
        let walled: RunLog = serde_json::from_value(serde_json::json!({
            "version": sprag_host::runs::RUN_LOG_VERSION,
            "runs": [{"id": 1, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                      "folds_by_reason": {"capacity": {"delivered": 3, "folded": 3}}}]
        }))
        .expect("a log from the daemon driving the loop today");
        let wall = folds_lines(&[(here.clone(), walled)], None).join("\n");
        assert!(
            wall.contains("NO population") && !wall.contains("landing(s) refute the axis"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴: a landing count over a population of zero is a zero \
             that reads as *the axis survived*. The population has to be stated instead — rule 6 \
             at the mouth, and the failure this item has walked into more than any other. \
             Got:\n{wall}",
        );

        // ── ⑤ AND NOTHING READ IS A REFUSAL ──
        let refused = folds(vec!["/nonexistent/nothing.runs.json".to_owned()])
            .expect_err("⛔ a log that cannot be read is a REFUSAL, never an empty table");
        assert_eq!(refused.kind(), io::ErrorKind::NotFound);
        assert!(
            refused.to_string().contains("nothing folded"),
            "⚠⚠ and it says which of the two silences this is, because *nothing was read* and \
             *nothing folded* are opposite facts that print alike: {refused}",
        );
        let extra = folds(vec!["a".to_owned(), "b".to_owned()])
            .expect_err("⚠⚠ two logs at once is refused rather than half-honoured");
        assert!(
            extra.to_string().contains("one at a time"),
            "⚠ and the refusal says what the shape is: {extra}",
        );

        // ── ⑥ AND A SWEPT ANSWER NAMES THE DIRECTORY IT IS ABOUT — `waits`' measurement, verbatim ──
        let swept = folds_lines(&[(here, log)], Some(std::path::Path::new("/state/sprag")));
        assert!(
            swept.first().is_some_and(
                |line| line.contains("/state/sprag") && line.contains("XDG_STATE_HOME")
            ),
            "⛔⛔⛔⛔⛔ A SWEPT TABLE DID NOT SAY WHICH DIRECTORY IT SWEPT. Measured \
             2026-09-05T08:47:23Z one verb over: the same command answered about the loop's 229 \
             runs with `XDG_STATE_HOME` set and about 62 integration-test runs without it, both \
             tables long and confident. Got: {swept:?}",
        );
        assert!(
            !said.contains("XDG_STATE_HOME"),
            "⚠⚠ AND A NAMED LOG CARRIES NO SUCH WARNING: the caller said where. Got:\n{said}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE TWO ENDS A CONVERSATION CAN BE ON ARE TOLD APART, AND A SHELL STILL GETS THE
    /// HALF THAT IS ANSWERABLE** — register item 865's ⑷.
    ///
    /// # What this holds that the integration gate cannot
    ///
    /// `cli.rs` drives a real daemon and reads the printed sentences, so it can only ask for
    /// SUBSTRINGS. This asks the classifier itself, over rows built here, so a build that answered
    /// the wrong END — the difference between *go and stop it, it is yours* and *this is being done
    /// to you* — goes red on the arm rather than on a needle.
    ///
    /// # ⚠⚠ The three sentences must DIFFER, and that is the point of the split
    ///
    /// A `Stake` whose arms all read the same is a distinction that makes no difference to the
    /// person reading, which is the defect item 827's gate was built for one axis over. ⚠ And the
    /// FOURTH case — on neither end — is [`None`] rather than a variant, because the verb answers
    /// it once for the whole listing (*nothing has you on either end*) and not per run.
    #[test]
    fn a_conversation_is_told_which_end_of_a_run_it_is_on() {
        const ME: &str = "3f2c9a17-0000-4000-8000-0000000008ff";
        const SOMEBODY_ELSE: &str = "3f2c9a17-0000-4000-8000-000000000900";
        const SEAT: u64 = 807;

        let row = |asker: Option<&str>, driving: Option<u64>, status: &str| -> Value {
            let mut run = serde_json::json!({ "id": 1, "state": { "status": status } });
            if let Some(asker) = asker {
                run[sprag_host::plugins::RUN_ASKED_BY_KEY] = serde_json::json!(asker);
            }
            if let Some(pane) = driving {
                run[sprag_host::plugins::RUN_DRIVING_KEY] = serde_json::json!(pane);
            }
            run
        };
        let running = sprag_host::plugins::RunStatus::Running.wire_str();

        assert_eq!(
            Stake::of(&row(Some(ME), None, running), Some(ME), SEAT),
            Some(Stake::Asked),
            "⛔ a run this conversation asked for is one it may stop",
        );
        assert_eq!(
            Stake::of(
                &row(Some(SOMEBODY_ELSE), Some(SEAT), running),
                Some(ME),
                SEAT
            ),
            Some(Stake::Driven),
            "⛔⛔ REGISTER ITEM 865 ⑷: a run driving this caller's pane is one it is ON, and it is \
             NOT the same end as having asked for it — a watcher that could not tell these apart \
             had to answer `I cannot say whether it is mine`",
        );
        assert_eq!(
            Stake::of(&row(Some(ME), Some(SEAT), running), Some(ME), SEAT),
            Some(Stake::Both),
            "⚠⚠ asking for a run onto one's OWN pane puts a conversation on both ends at once",
        );
        assert_eq!(
            Stake::of(
                &row(Some(SOMEBODY_ELSE), Some(999), running),
                Some(ME),
                SEAT
            ),
            None,
            "⚠ and a run this conversation is on neither end of is None — the answer that lets a \
             caller say `not mine` rather than `I cannot tell`",
        );

        // ⛔⛔⛔ A SHELL PANE STILL GETS THE DRIVEN HALF. `orchestrator` runs drive shells, and a
        // version of this that refused a seatless caller outright would answer *nothing* about a
        // pane something is demonstrably typing into.
        assert_eq!(
            Stake::of(&row(Some(ME), Some(SEAT), running), None, SEAT),
            Some(Stake::Driven),
            "⛔ a pane with no conversation is still driven, and `me` being absent must not take \
             that answer away",
        );
        assert_eq!(
            Stake::of(&row(Some(ME), None, running), None, SEAT),
            None,
            "⚠ ...and it can be on no ASKING end, because it has no conversation to have asked",
        );

        // ⛔⛔⛔⛔ AND AN ENDED RUN IS NOT DRIVING ANYBODY — the daemon's own rule for the panes
        // listing's `driven` marker, applied here: `Progress::driving` still names the pane an
        // ended run drove, and reading it unfiltered would say somebody is driving a pane nobody
        // is. ⚠ The ASKING end carries no such filter on purpose — *whose work is this* is a
        // question an ended run answers perfectly well.
        for ended in ["done", "panicked", "interrupted"] {
            assert_eq!(
                Stake::of(&row(Some(SOMEBODY_ELSE), Some(SEAT), ended), Some(ME), SEAT),
                None,
                "⛔ a run that has ended `{ended}` is not driving this pane now",
            );
            assert_eq!(
                Stake::of(&row(Some(ME), Some(SEAT), ended), Some(ME), SEAT),
                Some(Stake::Asked),
                "⚠ ...while a run it ASKED for is still its own after that run ended",
            );
        }

        // ⚠⚠ THE CONTROL: three ends, three DIFFERENT sentences. Without it a build whose arms all
        // read alike satisfies every assertion above and publishes a split nobody can act on.
        let mut said: Vec<&str> = Stake::ALL.iter().map(|stake| stake.describe()).collect();
        said.sort_unstable();
        said.dedup();
        assert_eq!(
            said.len(),
            Stake::ALL.len(),
            "⛔ two ends say the same thing, so telling them apart changes nothing for the reader",
        );
    }

    /// ⛔⛔⛔⛔ **EVERY DETAIL CLAUSE REACHES THE SCREEN, AND LANDS WHERE THE READERS EXPECT** —
    /// register items 594, 591 and 601's residue, measured 2026-08-22 and paid 2026-08-23.
    ///
    /// # What was missing
    ///
    /// Each of those rounds put a fact on the wire and taught a renderer to print it. **Not one of
    /// them was held to printing it.** The gate above was built for a fourth clause and covers only
    /// that one, so three facts reached `sprag runs` with nothing able to notice if they stopped:
    /// what became of a person's stand-down, how many of a loop's prompts nobody can see, and
    /// whether anything independent verified what a run converged on.
    ///
    /// ⚠⚠⚠⚠⚠ **AND `render_run`'S OWN COMMENT NAMES THE FAILURE IT WAS OPEN TO** — *"a fact that
    /// reaches the wire and dies at the mouth somebody actually reads"*. A comment is not a gate.
    /// This repository has paid for that sentence more than once, and it kept being written into
    /// the very renderer that nothing was checking.
    ///
    /// ⚠⚠⚠ **THE POSITION IS PART OF THE CLAIM, not tidiness.** This project's own outer-loop
    /// watcher reads a run's STATUS as the line after the heading and its WALK as the block's last
    /// line. A clause that lands at either end silently moves a reader that already exists — so
    /// each one is asserted to be present, and to be neither of those two lines.
    ///
    /// ⚠⚠ **THE CONTROL IS THE SAME RUN WITH NO KEYS AT ALL.** Without it a renderer that printed
    /// four fixed sentences on every run would satisfy every assertion here.
    #[test]
    fn every_fact_a_run_publishes_beside_its_state_reaches_the_person_reading_it() {
        // (the wire key, a sentence only that clause could produce). The sentences are the shape
        // the host's own renderers compose, and each is distinctive enough that finding it in the
        // output cannot be an accident of some other line.
        let clauses: &[(&str, &str)] = &[
            (
                sprag_host::plugins::RUN_STOOD_DOWN_KEY,
                "a person asked this run to stand down and it converged, so it ended on its own \
                 terms and its work is banked",
            ),
            (
                sprag_host::plugins::RUN_CHECKS_KEY,
                "an independent check was shown this milestone and agreed",
            ),
            (
                sprag_host::plugins::RUN_CANCELLED_BY_KEY,
                "a person cancelled this run, so the turn it was in the middle of was thrown away",
            ),
            // ⛔⛔⛔ AND WHAT THE DOOR ACCEPTED — register item 719's second direction. This is the
            // whole of *`orchestrate` stops accepting a brief silently* for a person: the door
            // answers a run id and points at this row, so a size that dies here is a size nobody
            // ever reads. ⚠ Level and not journal: the journal is bounded and a brief is said once
            // at the start, so on the long churning runs this exists for it would be evicted.
            (
                sprag_host::plugins::RUN_BRIEFED_KEY,
                "briefed with 9025 bytes (reference is the largest at 7000)",
            ),
        ];

        let mut run = run_entry(&blocked_run(sprag_plugin::Refusal::NoConsent, 0));
        // ⚠ A WALK, for the reason the gate above records: a block with an empty journal ends on
        // whatever clause came last, so the positional assertions would be measuring nothing.
        run[sprag_host::plugins::RUN_JOURNAL_KEY] = serde_json::json!([{
            "iteration": 1,
            "cost": 245,
            "unit": "bytes",
            "verdict": "continue",
            "note": "Idle --Start--> Priming",
        }]);

        // ── THE CONTROL: no keys, so none of the sentences may appear ──
        let quiet = render_run(&run);
        for (key, sentence) in clauses {
            assert!(
                !quiet.contains(sentence),
                "⚠⚠⚠ THE CONTROL: a run publishing no `{key}` must say nothing about it. A clause \
                 printed unconditionally would make every assertion below pass while saying \
                 nothing about any run's facts: {quiet}",
            );
        }
        // ⚠⚠ THE DELIVERY PAIR IS ITS OWN CONTROL, because its clause is composed from two numeric
        // keys rather than carried as a sentence — `plugins::delivery_sentence` is the one reader.
        assert!(
            !quiet.contains("prompt"),
            "⚠⚠⚠ THE CONTROL for the delivery pair: a run that delivered nothing must not talk \
             about prompts at all: {quiet}",
        );
        // ⛔⛔⛔ AND THE SPLIT OF THOSE FOLDS IS ITS OWN CONTROL TOO — register item 856(1), for
        // the delivery pair's reason exactly: it is a TABLE the mouth reads, not a sentence the
        // host carried. A run that reflected nothing must say nothing about reflections.
        assert!(
            !quiet.contains("folds by why it reflected"),
            "⚠⚠⚠ THE CONTROL for the split: a run that reflected nothing has no comparison to \
             publish, and a table of empty rows beside runs with real ones is rule 6 the other way \
             up — the escape is not a pass, it is an invented population: {quiet}",
        );

        for (key, sentence) in clauses {
            run[*key] = Value::String((*sentence).to_owned());
        }
        run[sprag_host::plugins::RUN_DELIVERED_KEY] = serde_json::json!(14);
        run[sprag_host::plugins::RUN_FOLDED_KEY] = serde_json::json!(14);
        // ⛔⛔⛔⛔ AND THE SPLIT OF THOSE FOLDS — register item 856(1). Deleting its clause from
        // `render_run` was MEASURED on 2026-09-04 to leave every gate in `sprag-host` green apart
        // from the standing one, which is why this arm exists: the item's whole remaining debt is
        // *somebody reads the number*, so a table that stops at the row pays nothing.
        //
        // ⚠⚠ THE LANDING ROW IS IN THE FIXTURE ON PURPOSE. A run whose every reflection folded
        // would let a mouth that printed only folds pass — and printing only folds is the defect,
        // because item 856's own refutation is a reflection that LANDED.
        // ⛔⛔⛔⛔ AND THE HARDENINGS — register item 856(3). `capacity` hardens on the road with NO
        // FOLD, which is the shape this repository's runs 191, 194 and 197 took (measured
        // 2026-09-04: `folded: 0` for the whole run, and 197's own record names it as what killed
        // it — 191 ended `failed` too but states no reason, so it is in the population and not
        // attributed); `budget` hardens on neither, which is the control this item's own
        // refutation lives in.
        run[sprag_host::plugins::RUN_FOLDS_BY_REASON_KEY] = serde_json::json!({
            "capacity": {
                "delivered": 3,
                "folded": 3,
                "unasked_after_a_fold": 0,
                "unasked_on_the_pane": 1,
            },
            "budget": {
                "delivered": 4,
                "folded": 0,
                "unasked_after_a_fold": 0,
                "unasked_on_the_pane": 0,
            },
        });
        let delivered = sprag_host::plugins::delivery_sentence(&run)
            .expect("a run that delivered has a delivery sentence");
        let split = sprag_host::plugins::folds_by_reason_sentence(&run)
            .expect("a run that reflected has a split to say");
        assert!(
            split.contains("budget 0 of 4"),
            "⚠⚠⚠ THE PREMISE OF THE ARM BELOW: the composed sentence must carry the row that \
             LANDED, or *the mouth prints it* is a claim about a sentence that already dropped the \
             only shape able to refute item 856: {split:?}",
        );
        // ⛔⛔⛔⛔⛔ AND THE HARDENING WITH ITS ROAD — register item 856(3). A person reading a run
        // that stopped for them needs to know whether to walk to the pane or to the agent's own
        // record, and *1 unasked* alone cannot tell them.
        assert!(
            split.contains("1 unasked (0 after a fold, 1 with the prompt on the pane)"),
            "⚠⚠⚠⚠ THE PREMISE for item 856(3): the split a person reads must name the road the \
             hardening took, or the two opposite remedies arrive as one number: {split:?}",
        );
        // ⛔⛔⛔⛔⛔ **AND THE ORDINARY ROW IS *NOT* IN THIS SENTENCE, WHICH IS A DECISION AND NOW A
        // PREDICATE** — register item 856's widening. That table gained a row for the run's
        // ordinary traffic so its rows would sum to the run's totals, and this sentence
        // deliberately leaves it out: the comparison it exists for holds the PROMPT SHAPE constant
        // and varies only what put the loop there, and briefs and turn prompts are a different
        // shape. Printed beside `capacity` it invites exactly the reading item 856 was filed
        // about.
        //
        // ⚠⚠ Asserted rather than left in a comment, because a comment is what a later round
        // walking `Occasion::ALL` here would not fail. The row's own home is the arithmetic —
        // `Deliveries` against the split — which is gated in `sprag_plugin::outer`.
        let mut with_ordinary = run.clone();
        with_ordinary[sprag_host::plugins::RUN_FOLDS_BY_REASON_KEY]["ordinary"] = serde_json::json!({
            "delivered": 40,
            "folded": 7,
            "unasked_after_a_fold": 0,
            "unasked_on_the_pane": 2,
        });
        let widened = sprag_host::plugins::folds_by_reason_sentence(&with_ordinary)
            .expect("a run that reflected still has a split to say");
        assert_eq!(
            widened, split,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856: the ordinary row reached the sentence that compares \
             REASONS. A reader meeting `ordinary 7 of 40` beside `capacity 3 of 3` reads two \
             populations as one axis — the prompts differ in shape, not in what put the loop \
             there — which is the pooling this split was built to end. Widened {widened:?}, \
             reasons-only {split:?}",
        );
        // ⛔⛔⛔⛔⛔ AND HOW MANY OF THIS RUN'S PROMPTS BECAME A QUESTION — register item 856, the
        // number three separate instruments could not hold. The table names the two roads that sit
        // inside `made - folded` and are NOT landings (a peer that paints nothing, and a run that
        // ended between the typing and the submit), so a mouth that printed the subtraction would
        // say 9 of 10 where the truth is 6.
        run[sprag_host::plugins::RUN_DELIVERED_BY_ROAD_KEY] = serde_json::json!({
            "painted": 5,
            "echoed": 0,
            "account": 0,
            "let_go": 1,
            // ⛔⛔⛔⛔⛔ **THE EIGHTH ROAD, AND ITS ABSENCE HERE WAS A RED THAT SHIPPED** — register
            // item 889 added `Emptied` and this hand-written table was not updated, so
            // `delivered_by_road_sentence` answered `None` — *whole or nothing* working exactly as
            // designed — and the sentence a person reads vanished.
            //
            // ⚠⚠⚠ **NOTHING CAUGHT IT FOR THREE COMMITS BECAUSE THE LANE WAS NEVER RUN.** Every
            // suite this round was `-p sprag-host --lib`, and this test lives in the `--bin sprag`
            // target: a package's binary targets are not in its lib run. Register item 833 already
            // recorded that shape — *"run the package with NO target filter"* — and it cost this
            // one again.
            "emptied": 0,
            "unchecked": 3,
            "unasked": 1,
            "unproven": 0,
        });
        let said = render_run(&run);
        let lines: Vec<&str> = said.lines().collect();
        let landed = sprag_host::plugins::delivered_by_road_sentence(&run)
            .expect("a run that delivered has a landing count to say");
        assert!(
            landed.contains("6 of 10 prompts became a question"),
            "⚠⚠⚠ THE PREMISE OF THE ARM BELOW: the composed sentence must carry the LANDING count, \
             or *the mouth prints it* is a claim about a sentence that says nothing item 856 was \
             filed about: {landed:?}",
        );
        let expected: Vec<&str> = clauses
            .iter()
            .map(|(_, sentence)| *sentence)
            .chain(std::iter::once(delivered.as_str()))
            .chain(std::iter::once(split.as_str()))
            .chain(std::iter::once(landed.as_str()))
            .collect();

        for sentence in &expected {
            assert!(
                said.contains(sentence),
                "⛔⛔⛔ A FACT THIS RUN PUBLISHES DIES AT THE MOUTH A PERSON READS. The daemon put \
                 it on the wire and `sprag runs` does not print it, which is the exact failure \
                 `render_run`'s own comment warns about. Missing: {sentence:?}\nGot:\n{said}",
            );
            let at = lines
                .iter()
                .position(|line| line.contains(sentence))
                .expect("the sentence is in the output");
            assert!(
                at > 1,
                "⚠⚠⚠ AND NOT ON THE HEADING OR THE STATUS LINE UNDER IT — the repayment loop's \
                 watcher reads the status positionally, as the line after the heading, so a clause \
                 landing there changes what an existing reader is told. {sentence:?} is at line \
                 {at}:\n{said}",
            );
            assert!(
                at < lines.len() - 1,
                "⚠⚠⚠ NOR LAST, which is where that same watcher reads the walk. {sentence:?} is \
                 at line {at} of {}:\n{said}",
                lines.len(),
            );
        }
        assert!(
            lines[lines.len() - 1].contains("Idle --Start--> Priming"),
            "⚠⚠ and the WALK is still last, which is the other half of the same claim — a clause \
             that pushed it off the end would satisfy the assertions above while breaking the \
             reader they exist to protect:\n{said}",
        );
    }

    /// ⚠⚠ **EVERY REASON REACHES THE PERSON, including the one with no question at all.**
    ///
    /// Driven from the type's own published list, so a reason ADDED to it fails here until this
    /// mouth says it. `unreadable` is the arm that carries no menu — its remedy is a person, and printing
    /// nothing for it would be the state R366 built the word to stop being silent about.
    #[test]
    fn every_refusal_reaches_the_person_reading_the_run() {
        for word in sprag_plugin::Refusal::WIRE_WORDS {
            let why = sprag_plugin::Refusal::parse(word).expect("published");
            let said = render_run(&run_entry(&blocked_run(why, 0)));
            let sentence = sprag_host::plugins::refusal_sentence(word);
            assert!(
                said.contains(&sentence),
                "{word:?} must reach the person as its own sentence: {said}",
            );
            assert_eq!(
                why == sprag_plugin::Refusal::Unreadable,
                !said.contains("Do you want to proceed?"),
                "⚠ only `unreadable` has no question to show, and it must still say what to do \
                 about it ({word:?}): {said}",
            );
        }
    }

    /// ⚠⚠⚠ **A RUN THAT ANSWERED SOMETHING FOR YOU SAYS SO — WHILE IT IS STILL RUNNING.**
    ///
    /// An approval a person only learns about once the loop is over is one they could not have
    /// stopped. Both halves are asserted because the running one is the half that matters and the
    /// half a renderer is likeliest to omit: the outcome is what everybody remembers to print.
    ///
    /// ⚠ And a run that answered NOTHING says nothing, which is not the same key being absent —
    /// the wire always carries the count. Silence here is this renderer's choice, and a clause on
    /// every ordinary run would train a reader to skip the line that matters.
    /// ⛔⛔⛔⛔⛔ **AND THE NEEDLE IS THE CLAUSE, NOT THE WORD** — narrowed 2026-09-05, register
    /// items 903 and 901.
    ///
    /// The negative arm below asked whether the row contained `"answered"`, which is broader than
    /// the fact it guards: what must not appear on a run that answered nothing is the CLAUSE
    /// *answered N question(s) for you*, and the bare word belongs to any sentence that has cause
    /// to use it. Item 903 added one — a blocked run now says what stopped it being answered — and
    /// the gate refused it while the property it defends was never in question.
    ///
    /// ⇒ This is register item 901's shape exactly, and its remedy: **a needle wider than its
    /// subject silently takes away the freedom to name things**, and what gets fixed is the needle
    /// rather than the innocent sentence. `for you` is what the positive arm above already matches
    /// on, so the two halves now name one clause between them.
    #[test]
    fn a_run_that_answered_for_you_says_so_before_it_is_over() {
        let running = serde_json::json!({
            "id": 7,
            "label": "orchestrator pane=1",
            "state": {
                "status": "running",
                "iterations": 4,
                "cost": 12,
                "unit": "bytes",
                sprag_host::plugins::RUN_ANSWERED_KEY: 2,
            },
        });
        let said = render_run(&running);
        assert!(
            said.contains("answered 2 questions for you"),
            "⚠⚠⚠ mid-flight, while a person can still cancel it: {said}",
        );

        let done = render_run(&run_entry(&blocked_run(
            sprag_plugin::Refusal::NotOffered,
            1,
        )));
        assert!(
            said.contains("answered") && done.contains("answered 1 question for you"),
            "and in the outcome, singular: {done}",
        );
        assert!(
            !render_run(&run_entry(&blocked_run(
                sprag_plugin::Refusal::NotOffered,
                0
            )))
            .contains("for you"),
            "⚠ and a run that answered nothing says nothing — a clause on every ordinary run \
             trains a reader to skip the line that matters",
        );
    }
    // The DECLARATION side of the grammar, which only a test builds here: this binary reads a
    // published grammar off a socket and never declares one.
    use sprag_host::wire::{ArgGrammar, CallForm};

    /// Every shape of the GRANT columns — the ceiling beside a usage, the ceiling on its own, and
    /// the weight — because the absences are the whole reason the type has three arms.
    ///
    /// ⚠ Written when the debt question asked what R340 had left untested and the answer was *the
    /// sentences a person actually reads*. The type-level distinction between `Uncapped` and
    /// `NoController` was gated in `sprag-terminal`; **nothing gated that the two RENDER
    /// differently**, and a renderer that collapsed them would have told somebody on a host with no
    /// `memory` delegation that they had chosen not to set a ceiling.
    #[test]
    fn every_grant_column_says_which_of_its_shapes_it_is() {
        // Beside a usage, `of` supplies the DENOMINATOR and says nothing when there is none: the
        // usage column has already printed `(no memory controller)`, and a second sentence about
        // the same missing controller would be this surface agreeing with itself at twice the
        // width.
        assert_eq!(
            of(
                "6 MiB".to_owned(),
                Ceiling::At(512 * 1024 * 1024),
                footprint_ceiling
            ),
            "6 MiB of 512 MiB",
        );
        assert_eq!(
            of("6 MiB".to_owned(), Ceiling::Uncapped, footprint_ceiling),
            "6 MiB"
        );
        assert_eq!(
            of(
                "(no memory controller)".to_owned(),
                Ceiling::NoController,
                footprint_ceiling
            ),
            "(no memory controller)",
        );
        assert_eq!(
            of("5 processes".to_owned(), Ceiling::At(64), count_ceiling),
            "5 processes of 64"
        );

        // ALONE, on `grant`'s own line, silence is not available: a person who ran the verb and got
        // a blank could not tell a ceiling that was removed from one that never took.
        assert_eq!(ceiling(Ceiling::At(64), count_ceiling), "64");
        assert_eq!(ceiling(Ceiling::Uncapped, count_ceiling), "uncapped");
        assert_eq!(
            ceiling(Ceiling::NoController, count_ceiling),
            "(no controller)"
        );
        assert_ne!(
            ceiling(Ceiling::Uncapped, footprint_ceiling),
            ceiling(Ceiling::NoController, footprint_ceiling),
            "a pane nobody capped and a host that cannot cap read differently",
        );

        // The weight is the SETTING, never a predicted share of the machine — a nominal 10:100 was
        // measured at 18:82, so anything shaped like a percentage here would be false.
        assert_eq!(weight(Counted::Now(10)), "10");
        assert_eq!(weight(Counted::NoController), "(no cpu controller)");
        assert!(
            !weight(Counted::Now(10)).contains('%'),
            "a weight is not a percentage of anything",
        );
    }

    /// The four columns `resources` prints, each shape of each one.
    ///
    /// # Why the shell's renderers are gated separately from the agent's
    ///
    /// They are two registers for one fact — a column a person scans against a sentence an agent
    /// reads — and the agent's are gated in `sprag-mcp`. What must never differ is the MEANING, and
    /// the shapes where that could slip are the absences: "no rate yet" must not print as `0.00`,
    /// and a controller that never arrived must not print as `0 B`. Both are asserted here and both
    /// are asserted there.
    #[test]
    fn every_resource_column_says_which_of_its_shapes_it_is() {
        assert_eq!(
            held(Cpu::Held {
                millicores: 3990,
                over_ms: 2500
            }),
            "3.99 cores over 2.5s"
        );
        // A pane the daemon has seen once has no rate — and a zero here would read as an idle pane,
        // which is the one thing this whole reading exists to tell apart.
        assert_eq!(held(Cpu::Settling), "(no rate yet)");

        assert_eq!(
            waited(Waiting::Measured {
                avg10: sprag_terminal::Percent::from_hundredths(8869),
                avg60: sprag_terminal::Percent::NONE,
                avg300: sprag_terminal::Percent::NONE,
            }),
            "88.69%"
        );
        assert_eq!(waited(Waiting::NotAccounted), "(unaccounted)");

        assert_eq!(footprint(Counted::Now(512)), "512 B");
        assert_eq!(footprint(Counted::Now(6 * 1024 * 1024)), "6 MiB");
        assert_eq!(
            footprint(Counted::Now(3 * 1024 * 1024 * 1024 + (1 << 29))),
            "3.5 GiB"
        );
        assert_eq!(footprint(Counted::NoController), "(no memory controller)");

        assert_eq!(count(Counted::Now(1)), "1 process");
        assert_eq!(count(Counted::Now(65)), "65 processes");
        assert_eq!(count(Counted::NoController), "(no pids controller)");
    }

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
    /// The `-L`-at-the-edge line is the one R299 CHANGED: it printed `already on 0`, an answer
    /// to a question the caller had not asked. Its integration test asserted that string, so the
    /// suite agreed with the wrong sentence for as long as it existed.
    #[test]
    fn select_pane_says_which_of_the_four_things_happened() {
        let toward = |dir| SelectAsk::Toward { dir, from: None };
        assert_eq!(
            select_sentence(SelectHow::Moved, SelectAsk::Pane(PaneId(3)), 3),
            "selected 3",
            "the shape a script greps, unchanged",
        );
        assert_eq!(
            select_sentence(SelectHow::Moved, toward(PaneDir::Left), 3),
            "selected 3",
        );
        assert_eq!(
            select_sentence(SelectHow::AlreadyActive, SelectAsk::Pane(PaneId(0)), 0),
            "already on 0",
        );
        // The four LITERALS, not `format!("nothing {} 0", dir.beyond())` — which is what this
        // asserted on its first draft and is not a test: it formats the string it then compares, so
        // `beyond()` could return anything and it would still pass (proved by changing one phrase and
        // watching it stay green). The register carries the same defect for `list-keys`' `-r` column.
        assert_eq!(
            PaneDir::ALL.map(|dir| select_sentence(SelectHow::AtEdge, toward(dir), 0)),
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
            select_sentence(SelectHow::Untiled, toward(PaneDir::Up), 2),
            "2 is floating, so nothing is beside it in any direction; name a pane to move to",
            "and it advises only what THIS surface can do — no CLI verb docks a pane",
        );
        // A daemon answering a word its request could not produce degrades to the true half of it
        // rather than panicking in a rendering — the failure mode `list-keys`' flag table had.
        assert_eq!(
            select_sentence(SelectHow::AtEdge, SelectAsk::Pane(PaneId(5)), 5),
            "already on 5",
        );
    }

    /// All four `swap-pane` sentences, including the two a live daemon is hard to drive into —
    /// [`select_pane_says_which_of_the_four_things_happened`]'s rule one verb over, and here the
    /// need is sharper: until R301 this verb had ONE sentence for three different outcomes.
    #[test]
    fn swap_pane_says_which_of_the_four_things_happened() {
        let toward = |dir| SwapAsk::Toward {
            pane: Some(PaneId(3)),
            dir,
        };
        let with = SwapAsk::With {
            pane: Some(PaneId(3)),
            with: PaneId(5),
        };
        assert_eq!(
            swap_sentence(SwapHow::Swapped, with, 3, Some(5)),
            "swapped pane 3 with 5",
            "the shape a script greps, unchanged",
        );
        assert_eq!(
            swap_sentence(SwapHow::Swapped, toward(PaneDir::Left), 3, Some(5)),
            "swapped pane 3 with 5",
            "and a DIRECTION caller learns who it traded with — the id it never typed",
        );
        assert_eq!(
            swap_sentence(SwapHow::SamePane, with, 3, Some(3)),
            "pane 3 cannot trade places with itself",
        );
        // The four LITERALS, never `format!("nothing {} 3", dir.beyond())` — a sentence compared
        // against the string the test formatted itself is not a test, which R299 proved on this
        // verb's twin by changing a phrase and watching it stay green.
        assert_eq!(
            PaneDir::ALL.map(|dir| swap_sentence(SwapHow::AtEdge, toward(dir), 3, None)),
            [
                "nothing to the left of 3 to trade with",
                "nothing to the right of 3 to trade with",
                "nothing above 3 to trade with",
                "nothing below 3 to trade with",
            ]
            .map(str::to_owned),
        );
        assert_eq!(
            swap_sentence(SwapHow::Untiled, toward(PaneDir::Up), 3, None),
            "3 is floating, so nothing is beside it in any direction; name a pane to trade with",
            "the OTHER sentence the edge one used to cover, and it advises only what THIS surface \
             can do — no CLI verb docks a pane",
        );
        // A daemon answering a word its request could not produce degrades to the true half of it
        // rather than panicking in a rendering.
        assert_eq!(
            swap_sentence(SwapHow::AtEdge, with, 3, Some(5)),
            "pane 3 cannot trade places with itself",
        );
        assert_eq!(
            swap_sentence(SwapHow::Swapped, toward(PaneDir::Up), 3, None),
            "swapped pane 3",
        );
    }

    /// Every one of the five resize outcomes has its own sentence, and the CLAMPED move — which is
    /// not an outcome word at all — has a sixth.
    ///
    /// Pinned as a pure function for [`swap_sentence`]'s reason: a live daemon can be driven into
    /// three of these easily and into `zoomed` and `untiled` only with a setup per case, so the
    /// wording would otherwise be tested by whatever happened to be reachable.
    #[test]
    fn resize_pane_says_which_of_the_five_things_happened() {
        let ask = |dir, cells| ResizeAsk {
            pane: Some(PaneId(3)),
            dir,
            cells,
        };
        assert_eq!(
            resize_sentence(ResizeHow::Resized, ask(PaneDir::Right, 5), 3, 5),
            "moved pane 3's right boundary 5 cells",
            "the shape a script greps",
        );
        assert_eq!(
            resize_sentence(ResizeHow::Resized, ask(PaneDir::Left, 1), 3, 1),
            "moved pane 3's left boundary 1 cell",
            "and it counts in English",
        );
        assert_eq!(
            resize_sentence(ResizeHow::Resized, ask(PaneDir::Right, 40), 3, 7),
            "moved pane 3's right boundary 7 cells of the 40 asked for; it stopped at the last \
             cell the far side may keep",
            "THE FACT NO OUTCOME WORD CARRIES — it moved, and not as far as it was told to",
        );
        assert_eq!(
            resize_sentence(ResizeHow::AtMinimum, ask(PaneDir::Up, 2), 3, 0),
            "pane 3 not resized: the boundary is already as far up as it goes",
        );
        assert_eq!(
            resize_sentence(ResizeHow::AtEdge, ask(PaneDir::Down, 2), 3, 0),
            "pane 3 not resized: the pane spans the window that way, so there is no boundary to \
             move down",
            "a DIFFERENT fact from the minimum, with a different remedy",
        );
        assert_eq!(
            resize_sentence(ResizeHow::Untiled, ask(PaneDir::Left, 1), 3, 0),
            "pane 3 not resized: the pane is floating, so it has no boundaries to move",
        );
        assert_eq!(
            resize_sentence(ResizeHow::Zoomed, ask(PaneDir::Left, 1), 3, 0),
            "pane 3 not resized: the window is zoomed, so its arrangement is not on screen; \
             unzoom to resize",
        );
        // Every word renders, so a daemon answering one this build did not expect cannot reach a
        // formatter that panics — `swap_sentence`'s rule.
        for how in ResizeHow::ALL {
            for dir in PaneDir::ALL {
                assert!(!resize_sentence(how, ask(dir, 3), 3, 1).is_empty());
            }
        }
    }

    /// **Every kill prints what its cascade actually reached**, and an answer that cannot say so
    /// prints the bare subject rather than a guess — R309.
    ///
    /// One renderer for three verbs, so the LEVEL each one names is the only thing that differs.
    /// The last block is the one that matters most: a daemon older than the cascade answers these
    /// actions with no word at all, and a reader that defaulted to the cheapest link would tell
    /// somebody their session survived a kill that ended it. It says less instead of saying wrong.
    #[test]
    fn a_kill_prints_every_level_it_ended_and_never_guesses_one() {
        let answer = |word: &str| json!({ "ended": word });

        assert_eq!(
            killed_sentence("pane 3", &answer("pane"), Ended::Pane),
            "killed pane 3",
            "a kill that stopped where it was aimed says nothing more",
        );
        assert_eq!(
            killed_sentence("pane 3", &answer("window"), Ended::Pane),
            "killed pane 3 — the window went with it",
        );
        assert_eq!(
            killed_sentence("pane 3", &answer("session"), Ended::Pane),
            "killed pane 3 — the window went with it, and the session",
        );
        assert_eq!(
            killed_sentence("pane 3", &answer("server"), Ended::Pane),
            "killed pane 3 — the window went with it, and the session, and the server",
        );

        // The SAME word, a different verb, a different sentence — which is why the renderer takes
        // the level the caller typed rather than deriving it from the answer alone.
        assert_eq!(
            killed_sentence("logs", &answer("session"), Ended::Window),
            "killed logs — the session went with it",
        );
        assert_eq!(
            killed_sentence("work", &answer("session"), Ended::Session),
            "killed work",
            "a kill-session that ended a session did exactly what was asked",
        );
        assert_eq!(
            killed_sentence("work", &answer("server"), Ended::Session),
            "killed work — the server went with it",
        );

        // A DAEMON THAT CANNOT SAY. Both shapes: the pre-R309 `null`, and a word from some future
        // build this one does not know.
        assert_eq!(
            killed_sentence("pane 3", &Value::Null, Ended::Pane),
            "killed pane 3",
            "a daemon too old to cascade is not reported as one that cascaded no further",
        );
        assert_eq!(
            killed_sentence("pane 3", &answer("everything"), Ended::Pane),
            "killed pane 3",
            "and neither is a word this build has never heard of",
        );
    }

    /// The usage text names every flag `select-pane` PARSES — held against the flag constants
    /// themselves, so the two spellings cannot drift.
    ///
    /// A usage block is a second list of what a binary does, and until R312 nothing in the suite
    /// read it. **It stopped being a second list at R323**: the text comes off
    /// [`sprag_host::vocabulary::usage`] now, so *which verbs appear* is no longer a claim anyone
    /// can get wrong. What a verb's own FORM spells still is — an entry saying `-L|-R` while its
    /// parser takes four directions is exactly as wrong as the old const was — and that is what
    /// this still checks, one entry at a time.
    #[test]
    fn the_usage_text_names_the_flags_select_pane_parses() {
        let usage = sprag_host::vocabulary::usage();
        let line = usage
            .lines()
            .find(|line| line.contains("select-pane"))
            .expect("the usage names select-pane");
        assert!(
            line.contains(FROM_FLAG),
            "the flag the verb parses must appear where a user looks for it: {line}",
        );
        // The four directions, DERIVED: every token of the line is offered to the same parser the
        // verb uses, and what it recognises must be all four. Spelling `-L` here would make the
        // test a fifth copy of the table the keymap exists to hold once.
        let mut named: Vec<&str> = line
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter_map(sprag_host::keymap::direction_of)
            .map(PaneDir::wire_str)
            .collect();
        named.sort_unstable();
        named.dedup();
        let mut every: Vec<&str> = PaneDir::ALL.map(PaneDir::wire_str).to_vec();
        every.sort_unstable();
        assert_eq!(
            named, every,
            "the usage line must offer every direction the verb parses: {line}",
        );

        // The SWAP's line, by the same derivation — and one thing more: `-t` must be spelled
        // OPTIONAL on it, because the verb stopped requiring one at R311. A usage that says a flag
        // is mandatory when it is not sends a reader to type something they need not. It used to
        // be checked by which BLOCK the verb sat in (the packed text shared one `-t SESSION` tail
        // across a run of verbs); a line per verb states it per verb, which is what a reader gets
        // to rely on.
        let swap = usage
            .lines()
            .find(|line| line.trim_start().starts_with("swap-pane "))
            .expect("the usage names swap-pane");
        assert!(
            swap.contains("[-t SESSION]"),
            "swap-pane takes an OPTIONAL session, and its own line is where that is said: {swap}",
        );
        let mut swap_named: Vec<&str> = swap
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter_map(sprag_host::keymap::direction_of)
            .map(PaneDir::wire_str)
            .collect();
        swap_named.sort_unstable();
        swap_named.dedup();
        assert_eq!(
            swap_named, every,
            "the swap's usage line must offer every direction it parses: {swap}",
        );
    }

    /// The two sentences an ORIGIN changes, and it changes them because the pane the caller asked
    /// about stops being the pane the user is on.
    ///
    /// The subject is the ORIGIN and the printed id is the LANDED pane, so a rendering that reached
    /// for the wrong one would be caught by these two lines and by nothing else in the suite: with
    /// no origin the two are the same pane, which is exactly the fixture that cannot tell them
    /// apart.
    #[test]
    fn a_step_that_goes_nowhere_names_the_pane_it_was_measured_from() {
        let from_seven = |dir| SelectAsk::Toward {
            dir,
            from: Some(PaneId(7)),
        };
        assert_eq!(
            select_sentence(SelectHow::AtEdge, from_seven(PaneDir::Left), 2),
            "nothing to the left of 7",
            "the user is on 2 and 7 is the pane with nothing to its left — naming 2 would report \
             an edge nobody asked about",
        );
        assert_eq!(
            select_sentence(SelectHow::Untiled, from_seven(PaneDir::Up), 2),
            "7 is floating, so nothing is beside it in any direction; name a pane to move to",
        );
        // A step that landed back on the pane the user was already on — reachable ONLY with an
        // origin, and the answer is the plain one: nothing moved.
        assert_eq!(
            select_sentence(SelectHow::AlreadyActive, from_seven(PaneDir::Right), 2),
            "already on 2",
        );
        assert_eq!(
            select_sentence(SelectHow::Moved, from_seven(PaneDir::Right), 8),
            "selected 8",
            "a move names where the user IS; where it started is the caller's own argument",
        );
    }

    /// A slot the daemon does not serve reaches an operator as a SENTENCE with a remedy, not as a
    /// Rust enum variant — R283's finding, on the query path this time.
    ///
    /// The fault is the one a live daemon actually sends, captured rather than invented: R290 asked
    /// a running `sprag-term` for a path it has no slot for and read
    /// `{"code":-32602,"message":"Invalid params","data":"UnknownIntrospectPath"}` off the socket.
    /// An action a daemon has never heard of is reported as a daemon too OLD, and a genuine
    /// refusal of an action it DOES serve keeps the verb's own sentence.
    ///
    /// Both faults were CAPTURED from a parent-commit daemon during R297's skew run, not invented:
    /// `/sprag_mux/external/rename_session` answered `UnknownInvokePath` there while `rename_window`
    /// with a bad window answered `InvokeRejected`. Before this told them apart, `sprag
    /// rename-session` against that daemon said the new name was already taken — about a name no
    /// session held.
    ///
    /// What stays HERE is the pure mapping and the three faults it must NOT claim; that every
    /// acting verb reaches it is `every_acting_verb_explains_a_daemon_that_does_not_know_its_verb`
    /// in `tests/cli.rs`, which is the half this one structurally cannot see — and the half that
    /// came back twenty-one-of-twenty-four the first time it was run.
    #[test]
    fn an_unknown_action_is_reported_as_an_old_daemon_and_a_refusal_is_not() {
        let fault = |data: Value| RpcFault {
            code: -32602,
            message: "Invalid params".to_owned(),
            data: Some(data),
        };
        let path = mux_action_path(RENAME_SESSION_ACTION);
        let error = unknown_action(&path, &fault(json!("UnknownInvokePath")))
            .expect("an action this daemon lacks is explained");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            error.to_string(),
            format!(
                "this daemon does not perform /sprag_mux/external/rename_session — {}",
                sprag_host::wire::SKEW_REMEDY,
            ),
        );

        // THE CONTROL — the fault a daemon that HAS the verb sends when it refuses one, which
        // must keep the verb's own disjunction. Without this the message would be right for the
        // rare case and wrong for the common one.
        assert!(
            unknown_action(&path, &fault(json!("InvokeRejected"))).is_none(),
            "a real refusal keeps the verb's own words",
        );
        assert!(
            unknown_action(&path, &fault(json!("a session named UnknownInvokePath"))).is_none(),
            "and a mention is not the refusal",
        );
    }

    /// The live half is no longer missing. It said here for three rounds that it *"cannot be a
    /// standing test: this suite spawns the CURRENT daemon, which serves every slot this binary
    /// knows"* — true of the daemon and false of the harness. `tests/cli.rs`'s `StaleHost` is a peer
    /// that passes the handshake and serves NO address, and
    /// `every_slot_reader_explains_a_daemon_that_does_not_serve_it` drives twelve verbs through it.
    /// What stays here is the pure mapping, which that test cannot reach: the three faults this
    /// must NOT claim.
    #[test]
    fn an_unserved_slot_is_reported_as_an_old_daemon_and_nothing_else_is() {
        let fault = |data: Value| RpcFault {
            code: -32602,
            message: "Invalid params".to_owned(),
            data: Some(data),
        };
        let error = unknown_slot("/x/y.0", &fault(json!("UnknownIntrospectPath")))
            .expect("an unknown path is explained");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            error.to_string(),
            format!(
                "this daemon does not serve /x/y.0 — {}",
                sprag_host::wire::SKEW_REMEDY
            ),
        );

        // THE CONTROL, and the reason this is a `data` match rather than a substring one: a
        // different refusal is left alone, and so is a fault whose rendered line merely mentions it.
        // The FIRST of these is now load-bearing twice over: it is also what `session_exists` reads
        // to tell a refused SCOPE from an unknown ADDRESS, so a widening here would turn a missing
        // session into a claim about the daemon's age.
        assert!(
            unknown_slot("/x/y.0", &fault(json!("no session named \"x\""))).is_none(),
            "another refusal keeps its own words",
        );
        assert!(
            unknown_slot(
                "/x/y.0",
                &fault(json!("a pane named UnknownIntrospectPath"))
            )
            .is_none(),
            "and a mention is not the refusal",
        );
        assert!(
            unknown_slot(
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

    /// Each wire METHOD this binary speaks is spelled EXACTLY ONCE, so a reader or an actor added
    /// later cannot reach the daemon without passing the site where the skew sentence is decided.
    ///
    /// # Why a claim about the SOURCE is the right shape here
    ///
    /// Both seams — [`query_slot`] for reading and [`invoke_action`] for acting — protect a sentence
    /// an operator sees, and both rest on the same unstated property: that they are the only way out
    /// of this file. That property was written in [`query_raw`]'s docs as a fact ("the ONE place this
    /// binary names the query method") and enforced by nothing, which is this project's own
    /// *"a not-built sentence is a claim"*. The register carried the gap as *"a slot reader added
    /// later that forgets `query_slot` is caught by nothing"*.
    ///
    /// The two CLI sweeps in `tests/cli.rs` cannot close it: they drive the verbs they NAME, so a
    /// verb added next round is covered by neither until somebody remembers. This is the assertion
    /// that does not need remembering — the new call site fails it at the moment it is written,
    /// which is also the moment its author is looking at the alternative.
    ///
    /// The needle is BUILT rather than written, so this test's own assertion is not one of the
    /// spellings it counts — which is the difference between a ratchet and a test that passes
    /// because it contains the thing it is looking for.
    #[test]
    fn every_wire_method_this_binary_speaks_is_spelled_once() {
        let source = include_str!("sprag.rs");
        for (method, seam) in [
            ("query", "query_raw"),
            ("invoke", "invoke_action"),
            ("revision", "wait_for_output"),
        ] {
            let spelling = format!("\"scene/{method}\"");
            let spelled = source.matches(spelling.as_str()).count();
            assert_eq!(
                spelled, 1,
                "{spelling} belongs to {seam} alone: a second spelling is a call site that can \
                 reach the daemon without the sentence {seam} decides. Route it through the seam \
                 rather than widening this count.",
            );
        }

        // THE CONTROL, and it has to be able to fail: a needle this file does NOT contain must come
        // back zero. Without it a broken `matches` (or a needle built wrong) would report one
        // spelling of everything and pass by measuring nothing.
        assert_eq!(
            source
                .matches(format!("\"scene/{}\"", "nonesuch").as_str())
                .count(),
            0,
            "the count is a real count",
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
    /// ⚠⚠ **EVERY SURFACE THIS CRATE SERVES IS REACHABLE FROM `show-grammar`** — the gate that
    /// would have caught the plugin host being undiscoverable for two rounds.
    ///
    /// A verb's grammar is answered by the surface that SERVES it, so the door has to be pointed at
    /// one. It knew the multiplexer and a pane; the plugin host published its own `action_grammar`
    /// from the round a derived audit found it (R353) and no flag ever reached it — so the verb
    /// whose whole job is *"ask the daemon how to call this"* could not be pointed at the loop the
    /// README leads with.
    ///
    /// The list of flags is hand-written, because a surface's PATH is not a function of its tag
    /// (a pane's hangs under a `pane_<id>` container that has to be resolved first). Its COVERAGE
    /// is not: this derives from `SURFACES`, which is itself checked against the scene the daemon
    /// SERVES, so a fourth sprag-authored surface fails here rather than quietly becoming
    /// undiscoverable.
    ///
    /// ⚠ An UPSTREAM surface is deliberately not required: sprag does not publish a grammar for a
    /// pinion widget's verbs, so there is nothing for this verb to point at.
    #[test]
    fn every_surface_this_crate_serves_is_reachable_from_show_grammar() {
        let reachable: Vec<&str> = GrammarSurface::ALL.iter().map(|it| it.tag()).collect();
        let mut checked = 0;
        for surface in sprag_host::wire::SURFACES {
            if surface.author != sprag_rpc::grammar::SurfaceAuthor::Sprag {
                continue;
            }
            checked += 1;
            assert!(
                reachable.contains(&surface.tag),
                "{} serves verbs and publishes their grammar, and `show-grammar` cannot be pointed \
                 at it — add a flag to GrammarSurface, or the one verb for asking the daemon how to \
                 call something cannot be asked about this surface",
                surface.name,
            );
        }
        assert_eq!(
            checked,
            GrammarSurface::ALL.len(),
            "the flags and the surfaces are one-to-one; a flag naming a surface this crate does not \
             serve is a door onto nothing",
        );
    }

    /// ⚠⚠ **THE CLI'S OWN FLAGS AND THE DAEMON'S ARGUMENTS ARE ONE NAMESPACE, AND A COLLISION IS
    /// SAID RATHER THAN RESOLVED.**
    ///
    /// Every other flag of `orchestrate` is the daemon's, read off its published grammar.
    /// [`OWN_FLAGS`] are this command's. If a daemon ever publishes a run argument of one of those
    /// names the two meanings collide, and a binary that picked one would be guessing about a
    /// daemon it did not compile with — so it refuses and names the way round.
    ///
    /// Driven by handing the check a doctored publication, which is the only way this branch is
    /// reachable: no daemon in this workspace publishes such an argument, and
    /// `the_orchestrate_refusals_are_the_daemons_own_grammar` shows the real one does not.
    ///
    /// ⚠ EVERY flag in the table, not the one that was there first — register item 855, which
    /// added `--dry-run` beside `--wait`. A check that walks a table and a test that names one of
    /// its rows is a gate that stops covering the day the table grows, which is the shape a great
    /// many of this workspace's exemption arms had.
    #[test]
    fn a_daemon_argument_that_collides_with_one_of_this_commands_own_flags_is_refused() {
        const REAL: &[ArgGrammar] = &[
            ArgGrammar::open("pane", "int"),
            ArgGrammar::open("stimulus", "string"),
        ];
        let read = |args: &'static [ArgGrammar]| {
            sprag_rpc::PublishedForm::read(
                &CallForm::object(args).to_answer(),
                "a doctored publication",
            )
            .expect("it reads")
        };

        assert!(
            !OWN_FLAGS.is_empty(),
            "a table with no rows would pass every assertion below by iterating nothing",
        );
        for own in OWN_FLAGS {
            let (flag, instead) = (own.name, own.instead);
            // A doctored publication, declared as the wire declares one — no daemon serves it,
            // which is exactly why the branch needs a fixture rather than a run.
            let clashing = read(vec![ArgGrammar::open(flag, "int")].leak());
            let refusal =
                own_flag_collision(std::slice::from_ref(&clashing)).unwrap_or_else(|| {
                    panic!("a daemon publishing {flag:?} collides with this command's own flag")
                });
            assert!(
                refusal.contains(flag) && refusal.contains(instead),
                "the refusal for {flag:?} must name it and say what to do instead: {refusal}",
            );
        }

        // THE CONTROL, and it is the case that actually ships: the arguments a real `run` form
        // carries do not collide, so the check is silent and every flag reaches the fill.
        let real = read(REAL);
        assert!(own_flag_collision(std::slice::from_ref(&real)).is_none());
    }

    /// ⚠⚠ **THE USAGE DESCRIBES EVERY FLAG THIS COMMAND OWNS, AND IT IS BUILT FROM THE TABLE** —
    /// register item 864.
    ///
    /// # What was measured, and why it is item 855's own defect
    ///
    /// Item 855 added `--dry-run` beside `--wait` and made the collision check walk a TABLE — and
    /// then spelled both flags by hand in the usage. So `OWN_FLAGS` could gain a row that `--help`
    /// never mentioned, and a caller reading the only self-description this binary offers would not
    /// know the flag existed. That is a fact kept in a second place, which is exactly what item 855
    /// was filed on, reappearing inside 855's repair.
    ///
    /// # Three claims, and the third is the one a hand-written usage fails
    ///
    /// * the SUMMARY carries one bracketed flag per row, and NO MORE — a hand-written extra is as
    ///   much a second place as a missing one;
    /// * the LINES are one per row, each naming its flag and saying what it does;
    /// * **a table this binary has never seen still reaches the printed usage.**
    ///
    /// ⚠⚠ THE THIRD CLAIM IS SERVED A ROW THAT DOES NOT EXIST, and that is the whole design of this
    /// gate. Its first shape compared the usage against `OWN_FLAGS` itself and PASSED its own
    /// mutation (2026-09-03): a printer spelling today's two flags by hand produces character-for-
    /// character the string the builders produce, so no assertion over two rows can tell them
    /// apart. Only a THIRD row — which a hand-written line cannot contain and a walk over the
    /// argument cannot miss — separates the two.
    #[test]
    fn the_orchestrate_usage_describes_every_flag_this_command_owns() {
        const REQUIRED_ONLY: &[ArgGrammar] = &[
            ArgGrammar::open("plugin", "string"),
            ArgGrammar::open("pane", "int"),
        ];
        assert!(
            !OWN_FLAGS.is_empty(),
            "a table with no rows would pass every assertion below by iterating nothing",
        );

        // ── ① THE SUMMARY: one bracketed flag per row, and no more ──────────────────────────
        let summary = own_flag_summary(OWN_FLAGS);
        assert_eq!(
            summary.matches("[--").count(),
            OWN_FLAGS.len(),
            "the first usage line offers exactly the table's rows: {summary:?}",
        );

        // ── ② THE LINES: one per row, each naming its flag and what it does ─────────────────
        let lines = own_flag_lines(OWN_FLAGS);
        assert_eq!(
            lines.lines().count(),
            OWN_FLAGS.len(),
            "one line per row rather than a hand-written list: {lines:?}",
        );
        for own in OWN_FLAGS {
            assert!(
                summary.contains(&format!("[--{}]", own.name)),
                "the summary offers --{}: {summary:?}",
                own.name,
            );
            assert!(
                lines.contains(own.name) && lines.contains(own.does),
                "the paragraph says what --{} does: {lines:?}",
                own.name,
            );
        }

        // ── ③ A ROW NO PRINTER KNOWS REACHES THE USAGE ─────────────────────────────────────
        //
        // The doctored publication's arguments are all required, so nothing the DAEMON published
        // can be mistaken for one of this command's own optional flags.
        const INVENTED: &str = "a-flag-no-build-of-this-binary-has";
        let doctored = &[
            OwnFlag {
                name: INVENTED,
                does: "exists only in this test, and only a walk over the table can print it",
                instead: "nothing — no daemon publishes this",
            },
            OwnFlag {
                name: WAIT_FLAG,
                does: "the shipping row beside it, so the walk is not the only thing that matches",
                instead: "parking until a run ends.",
            },
        ];
        let form = sprag_rpc::PublishedForm::read(
            &CallForm::object(REQUIRED_ONLY).to_answer(),
            "a doctored publication",
        )
        .expect("it reads");
        let usage = orchestrate_usage(std::slice::from_ref(&form), &None, doctored);
        assert!(
            usage.contains(&own_flag_summary(doctored))
                && usage.contains(&own_flag_lines(doctored)),
            "⚠⚠ THE PRINTER SPELLS ITS OWN FLAGS INSTEAD OF WALKING THE TABLE: a row it was handed \
             is missing from the usage a caller reads. {usage:?}",
        );
        assert!(
            usage.contains(INVENTED),
            "and the invented row is the proof, by name: {usage:?}",
        );
        assert!(
            !usage.contains(DRY_RUN_FLAG),
            "⚠ THE CONTROL: handed a table without it, the usage must not print --{DRY_RUN_FLAG} \
             from somewhere else. {usage:?}",
        );
    }

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
