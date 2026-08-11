//! `sprag-mcp` — the **agent-facing surface** over the sprag terminal host.
//!
//! sprag's north star is "an AI reads and drives the terminal as *data*": the
//! headless host (`sprag-term`) owns every pane's PTY and serves a JSON-RPC wire
//! (pane list, cell/full-text read, key/text input) over an always-on Unix socket.
//! That capability was already complete — but an agent *dropped into a pane* had no
//! way to DISCOVER it: it saw only an opaque `SPRAG_HOST_RPC_SOCK` env var and would
//! have to reverse-engineer the wire from source. This binary closes that gap.
//!
//! It is a [Model Context Protocol](https://modelcontextprotocol.io) **stdio
//! server**: Claude Code (or any MCP client) spawns it and it speaks newline-delimited
//! JSON-RPC 2.0 on stdin/stdout. It advertises self-describing tools, so an agent *immediately*
//! understands "read/write a sibling pane" without reading any sprag source.
//!
//! **WHICH tools is not written here, and that is R335's correction of R138's.** This paragraph
//! used to NAME them, on the argument [`tools_list`] still makes about counts — *a number kept in
//! prose goes stale silently, and this one had*. Naming them was the same defect a paragraph
//! longer: measured at `9727042` the list held eighteen of twenty-nine, so the eleven it omitted
//! were invisible to a reader who trusted it, and nothing could have said so.
//!
//! The roster is [`tools_list`], and what a tool MEANS is
//! [`sprag_host::vocabulary`] — one verb, up to three mouths (a shell, a keystroke, an agent), with
//! `the_roster_is_exactly_what_the_vocabulary_declares` holding this file's roster against that
//! table in both directions. A tool added here without a verb fails; a verb whose tool is missing
//! here fails. That is the whole reason this crate depends on `sprag-host`.
//!
//! The two `agent_*` tools are the surface for the one fact an agent cannot read off a
//! sibling's screen without interpreting it: whether the AI in that pane is waiting for a human
//! (H3). They report the daemon's own verdict, so two agents watching one pane agree.
//!
//! ## A pane NUMBER is positional, so the handle an agent holds is a NAME
//!
//! Every tool here addresses a pane by its 1-based number in `list_panes`, and that number moves:
//! closing any earlier pane shifts every number after it. So a number an agent remembered can come
//! to name a DIFFERENT pane, and the `write_pane` that follows succeeds against the wrong subject —
//! the worst answer a surface can give, because nothing about it looks like a failure.
//!
//! The stable handle could not be the host id this surface already prints, because a number and an
//! id are both integers and one argument cannot carry the two without a mode flag. A NAME is a
//! string, so **JSON's own types discriminate it**: `pane: 3` is the third pane and `pane: "build"`
//! is the pane called build. That is why the handle is a name ([`sprag_terminal::PaneName`], which
//! refuses an all-digit one for exactly this reason), and why [`tool_open_pane`] takes one at birth
//! — an agent that names its work pane never has to hold a number at all.
//!
//! ## The agent's OWN pane, and why it is the only structural write here
//!
//! [`tool_open_pane`], [`tool_close_pane`] and [`tool_rename_pane`] are the one place this surface
//! CHANGES the set of panes (or what a person reads on one) rather than reading or typing into it. Everything else here works on a pane a person
//! opened, which left "run the build over there and wait for it" — the workflow `pane_processes`,
//! `pane_job_changed` and `wait_for_change` were built for — with no first step an agent could take.
//!
//! There is deliberately no `move`, `swap` or `zoom` tool: those decide what a HUMAN looks at, and
//! an agent has no basis for the decision. Opening a pane to work in is not that — it is the agent's
//! own workbench, appended without an opinion about the arrangement.
//!
//! What makes the destructive half safe to hand an agent is that the daemon records WHO ASKED for
//! each pane ([`sprag_terminal::Pane::opened_by`]), so `close_pane` — and `rename_pane`, on the
//! same argument, since a pane's name is what a PERSON reads on it — can refuse every pane its
//! caller did not open. That is an ergonomic guard rather than a boundary — an agent that can `write_pane`
//! into a shell can run `sprag kill-pane` — and the mistake it prevents is the one that actually
//! happens: a mis-resolved pane number ending a person's editor and taking its scrollback with it.
//!
//! `list_panes` answers WHO is in the terminal, [`tool_pane_layout`] answers WHERE they sit, and
//! [`tool_pane_processes`] answers WHAT each one is RUNNING — the same three-way split the daemon
//! publishes and the `sprag` CLI exposes. WHERE is what lets an agent choose a pane the way a person
//! describes one — by position — and it carries the daemon's own adjacency plus a mark on the pane
//! this server is itself running in, without which a direction has nothing to be relative to. WHAT
//! is the one thing a pane's TEXT cannot be read for: `list_panes` carries the label a pane was
//! spawned with and never revisits, output that mattered may have scrolled away, and a silent build
//! looks exactly like an idle prompt.
//!
//! `select_pane` is the one tool whose subject is the PERSON rather than a pane: it moves where the
//! user's keystrokes go (H7's active pane), so an agent that has prepared something to look at can
//! put them on it. `list_panes` marks that pane, which is also how an agent learns where a human is
//! working before typing somewhere else. It takes a pane OR a DIRECTION, and the direction is what
//! joins WHERE to the move: adjacency is a fact only the daemon holds atomically, so an agent that
//! read `pane_layout` and then selected a number would assemble one action out of two instants.
//!
//! Each tool call bridges to the host wire via [`sprag_rpc::HostConn`], addressing panes with the
//! [`sprag_host::wire`] path SSOT.
//!
//! ## Locating the host (works in any pane, no per-instance config)
//!
//! The host socket path is per-GUI-instance (`sprag-gui-host-<pid>.sock`), so it
//! cannot be hard-coded in a global MCP registration. [`host_sock`] resolves it in
//! two layers, so the server self-configures regardless of whether the MCP client
//! forwards the env var:
//!
//! 1. This process's own `SPRAG_HOST_RPC_SOCK` (set if the client inherits/forwards
//!    the pane shell's environment).
//! 2. Failing that, walk the `/proc` parent-process chain from our own PID and read
//!    the first ancestor that carries `SPRAG_HOST_RPC_SOCK` in its environment — the
//!    `sprag-term` host is always an ancestor of a pane's processes, so its socket is
//!    discoverable from inside its own process tree with no configuration at all.
//!
//! When neither resolves (the agent is NOT inside a sprag pane), every tool returns a
//! clear "not inside a sprag terminal" error instead of failing opaquely.
//!
//! ## Protocol
//!
//! Only the minimal server surface is implemented: `initialize` (echoing the client's
//! `protocolVersion` for forward-compat), `tools/list`, `tools/call`, and `ping`;
//! notifications (`notifications/initialized`, `notifications/cancelled`) are accepted
//! and dropped. stdout carries ONLY protocol JSON — all diagnostics go to stderr via
//! `tracing` (env `SPRAG_LOG`, default `warn`).

// A binary crate: `cargo doc` builds it with private items, and the crate-root doc above links
// to the bin's own internals (e.g. [`host_sock`]) as a navigable map. `private_intra_doc_links`
// guards LIBRARY public-API docs, which publish without private items; a bin has no such
// surface, so the lint is a structural false positive here (mirrors `sprag-gui`).
#![allow(rustdoc::private_intra_doc_links)]

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use sprag_host::events::EventFilter;
use sprag_host::pane_address::{
    NamedPane, PaneListing, ambiguous_pane_name, unknown_pane_name_with,
};
use sprag_host::report::Severity;
use sprag_host::shellword::shell_quote;
use sprag_host::vocabulary::{Agent, Verb};
use sprag_host::window::SizeRequest;
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, BREAK_PANE_ACTION, CLOSE_ACTION, DISPLAY_MESSAGE_ACTION, DOCTOR_WINDOW,
    ENDED_KEY, EVENTS_WAIT_METHOD, FULL_TEXT_SLOT, GRANT_PANE_ACTION, JOIN_PANE_ACTION, JoinAsk,
    KEY_ACTION, KILL_WINDOW_ACTION, LAST_COMMAND_SLOT, LAYOUT_SLOT, LINKS_SLOT, MOVE_PANE_ACTION,
    NEW_WINDOW_ACTION, OUTCOME_KEY, PANES_SLOT, PaneProcessesWire, PaneResourcesWire,
    RENAME_PANE_ACTION, RENAME_WINDOW_ACTION, RESIZE_PANE_ACTION, RESIZE_WINDOW_ACTION, ResizeAsk,
    ResizeHow, ResizeWindowAsk, SELECT_PANE_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT,
    SINCE_PARAM, SPAWN_ACTION, SWAP_PANE_ACTION, SelectAsk, SelectHow, SelectWindowAsk, SwapAsk,
    SwapHow, TEXT_ACTION, WINDOWS_SLOT, WindowBirthAsk, WindowPin, WindowRef, ZOOM_PANE_ACTION,
    doctor_over, find_slot_for, pane_processes_at, pane_resources_at, regex_slot_for, settled,
};
use sprag_host::{ClientSize, PANE_ENV_VAR, PaneFind, mux_action_path, pane_input_path};
use sprag_rpc::{
    CallError, HostConn, INVALID_PARAMS, NEEDLE_PARAM, PANE_PARAM, PANE_WAIT_OUTPUT_METHOD,
    PATTERN_PARAM,
};
use sprag_terminal::{
    Ceiling, Counted, Cpu, Diagnosis, Ended, LayoutSnapshot, OrderStep, PaneDir, PaneId, SplitDir,
    SplitSide, Taken, Verdict, Waiting, WindowInfo, arrangement,
};

/// The env var the host sets on the pane shells it spawns (and thus on their
/// descendants) — [`sprag_host`]'s socket policy path key.
const SOCK_ENV: &str = "SPRAG_HOST_RPC_SOCK";

/// How long a tool call waits for the host socket to accept before erroring.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// The MCP protocol version advertised when the client's `initialize` omits one.
/// (Normally we echo the client's requested version for maximum compatibility.)
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

fn main() {
    init_tracing();
    tracing::info!(target: "sprag_mcp", "sprag-mcp starting (stdio)");
    if let Err(error) = serve() {
        tracing::error!(target: "sprag_mcp", %error, "server loop ended on I/O error");
        std::process::exit(1);
    }
    tracing::info!(target: "sprag_mcp", "stdin closed; exiting");
}

/// Install the stderr `tracing` subscriber. stdout is the MCP wire, so logs MUST go
/// to stderr; level control is `SPRAG_LOG` (RUST_LOG syntax), default `warn`.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("SPRAG_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_writer(io::stderr)
        .with_env_filter(filter)
        .try_init();
}

/// The read/dispatch loop: one JSON-RPC message per stdin line until EOF.
fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(()); // EOF — the client closed the pipe; exit gracefully.
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(target: "sprag_mcp", %error, "dropping non-JSON line");
                continue;
            }
        };
        dispatch(&mut out, &message)?;
    }
}

/// Route one parsed message. Requests (with `id`) get a response; notifications
/// (no `id`) are handled silently.
fn dispatch(out: &mut impl Write, message: &Value) -> io::Result<()> {
    let id = message.get("id").cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match method {
        "initialize" => respond(out, id, handle_initialize(message)),
        "tools/list" => respond(out, id, tools_list()),
        "tools/call" => respond(out, id, handle_tools_call(message)),
        "ping" => respond(out, id, json!({})),
        // Notifications (no id) — accept and drop, never reply.
        _ if id.is_none() => {
            tracing::debug!(target: "sprag_mcp", method, "notification ignored");
            Ok(())
        }
        other => respond_error(out, id, -32601, &format!("method not found: {other}")),
    }
}

// ---- JSON-RPC response writers -------------------------------------------------

/// Write one newline-delimited JSON message to stdout and flush.
fn write_message(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{value}")?;
    out.flush()
}

/// Send a successful JSON-RPC `result` echoing the request `id`.
fn respond(out: &mut impl Write, id: Option<Value>, result: Value) -> io::Result<()> {
    let Some(id) = id else {
        tracing::warn!(target: "sprag_mcp", "request lacked an id; cannot respond");
        return Ok(());
    };
    write_message(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
}

/// Send a JSON-RPC protocol `error` (e.g. unknown method / bad params).
fn respond_error(
    out: &mut impl Write,
    id: Option<Value>,
    code: i64,
    message: &str,
) -> io::Result<()> {
    let Some(id) = id else { return Ok(()) };
    write_message(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    )
}

// ---- Handshake + tool catalog --------------------------------------------------

/// `initialize` result — echo the client's `protocolVersion`, advertise a tools-only
/// server, and hand the agent an `instructions` primer so it grasps the surface at once.
fn handle_initialize(message: &Value) -> Value {
    let protocol_version = message
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "sprag-mcp", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "You are running inside a pane of a sprag terminal. These tools let \
            you observe and drive the terminal as data. \
            Call `list_panes` FIRST to see the pane numbers (1 = the first pane); \"pane 2\" \
            means the second pane in that list. A number is POSITIONAL — closing an earlier pane \
            shifts every number after it — so for any pane you will come back to, use its NAME \
            instead: every `pane` argument here takes a name as well as a number. \
            `list_panes` answers about YOUR WINDOW only. A sprag session holds several, and \
            `list_windows` is what tells you so — the other windows, their panes, and the names \
            those panes carry. A pane NAME reaches ANY window of this session, at EVERY tool here \
            that takes a `pane`: name a pane once and you can read it, search it, type into it, \
            wait on it and ask about its agent wherever it is. A pane NUMBER never reaches past \
            your window — it means the Nth row of `list_panes`, and `list_panes` is yours. \
            `list_sessions` answers what else this daemon holds, which is what the \
            session changes you can wait for are about. \
            READ a pane: `read_pane` (its screen and scrollback), `read_last_command` (just \
            the last command and its result), `read_pane_links` and `read_pane_images` (what \
            it shows that is not text), `find_in_pane` and `regex_in_pane` (search it). \
            Ask WHERE the panes are with `pane_layout` — it draws the arrangement, marks the \
            pane YOU are in, and says which pane is left, right, above and below each one, so \
            \"the pane next to mine\" resolves to a number. Ask WHAT each one is running with \
            `pane_processes`, which is the operating system's answer and not a guess from the \
            pane's text, and what that is COSTING with `pane_resources` — the cores each pane \
            holds and how much of the recent past it spent waiting for cores it did not get, \
            which is how to tell your own work being heavy from another pane starving you. \
            When YOUR OWN pane is the one taking the machine, `grant_pane` gives it a \
            CPU weight, a memory ceiling and a process ceiling, so the panes waiting on \
            you can get on. It acts only on a pane you opened, like every other tool that \
            changes something. A weight is not a cap: a held-back pane still takes the \
            whole machine when nothing else wants it, so this slows nobody down on an \
            idle box. When the greedy pane is a person's, say what you measured instead. \
            When EVERY pane is starved and none is greedy, the machine itself has less to give \
            than it should, and `machine_health` says why: a fixed set of checks on the machine, \
            each printing the value it measured beside its verdict. Most of what it finds is not \
            sprag's — a compiler cache the shells walk past, memory gone to swap, something \
            outside this terminal taking the cores. It detects only, so tell the person what it \
            found rather than acting on it. \
            DRIVE a pane with `write_pane` (type a command) and `send_keys` (named keys and \
            chords). \
            Instead of polling, WAIT: `wait_for_change` for the one change you name — a \
            job starting or finishing, a pane opening or closing, an agent's state moving — and \
            `wait_for_output` for a pane PRINTING text you name, which is how you run something \
            over there and are told the moment it says what you were waiting for. \
            About a sibling AI: `agent_state` says whether it is working, waiting for a human, \
            or at rest, and `agent_explain` says why. \
            For your OWN work, `open_pane` gives you a new pane to run things in without taking \
            over one a person is reading — name it there, and address it by that name afterwards \
            — `rename_pane` changes that name later, `swap_pane` moves it to a different place in \
            the arrangement, `resize_pane` makes it wider or taller when what you are reading \
            wraps, and `close_pane` closes it. \
            You can also move a pane you opened OUT of the window it is in and BETWEEN windows: \
            `break_pane` gives it a window of its own without moving the user, `join_pane` puts it \
            into a window you name, `move_pane` puts it beside a particular pane on an axis you \
            choose, and `zoom_pane` makes it fill its window when what you are reading needs every \
            column there is. Every one of these acts ONLY on a pane you opened; a person's pane is \
            refused, because their names, their arrangement and how big their panes are are theirs. \
            `select_pane` moves where the USER is typing, so use it only when you have \
            something for them to look at. \
            To SAY something to the person rather than show them something, `display_message` puts \
            one line on the status line of every window attached to this terminal — the only way \
            to reach somebody who is looking at a different pane. Use it when you have finished \
            what they were waiting on or when you are blocked and need them, not to narrate; send \
            it as an `alert` when missing it would make the message useless, since a note goes \
            away by itself and an alert waits for a keypress. Its answer says WHO saw it, and \
            \"nobody\" is one of the answers: if no window is attached, the person was not told \
            and you must leave the evidence somewhere they will find it. \
            For work that needs MORE ROOM than a pane beside somebody, `open_window` gives you a \
            whole screenful of your own — created WITHOUT moving the user, who cannot see it until \
            you call `select_window`. That split is deliberate: making a place and showing it are \
            two acts, and only the second takes a person's screen. `rename_window` and \
            `close_window` finish the job, and `resize_window` forces its SHAPE when what you are \
            reading needs more columns than the people watching can give it — all three act ONLY \
            on a window you opened, a person's window is refused, and so is closing the session's \
            last one. A forced size is only laid out while the `window-size` option is `manual`, \
            so read what `resize_window` answers rather than assuming the columns moved. \
            When the work is a LOOP rather than a single act — prompt something, read what it \
            said, decide, prompt again — do not run that loop in your own turns. `orchestrate` \
            runs it inside the platform, which is the only way it is BOUNDED: it stops at an \
            iteration ceiling and at a cost ceiling in the run's own unit (injected bytes, or \
            model tokens), it ends each turn on the sibling agent's MEASURED state rather than \
            on a timer, and `cancel_run` stops it between steps so the pane is left readable. A \
            loop you drive yourself has none of those, and nothing can stop it if it does not \
            converge. It returns a run id at once; `list_runs` says how it ended, and it still \
            says so if the run finished while you were doing something else. Every pane a run \
            touches must be one you opened, and the runs you see are the ones you started. \
            If a tool reports it is not inside a sprag terminal, these tools do not apply to \
            this session."
    })
}

/// The self-describing tool roster. Descriptions are written for an agent so a request like "type
/// xxx into pane 2" maps directly onto the `write_pane` tool, and "is the agent in pane 2 done?"
/// onto `agent_state`.
///
/// The count is deliberately not stated here: it was wrong (it said seven while there were nine),
/// which is what a number maintained in prose does. The roster itself is below and the crate-root doc
/// names the tools rather than counting them.
fn tools_list() -> Value {
    // A NUMBER or a NAME, and the JSON type is what tells them apart — see `pane_target`. The
    // description leads with the hazard rather than with the syntax, because the hazard is what a
    // caller cannot discover: a number that worked a moment ago can silently name a different pane.
    let pane_arg = json!({
        "type": ["integer", "string"],
        "minimum": 1,
        "description": "Which pane. A NUMBER is the 1-based position in list_panes (1 = the \
            first pane) — convenient, but POSITIONAL: closing any earlier pane shifts every \
            number after it, so a number you remembered can come to mean a different pane and \
            the write will succeed against the wrong one. A STRING is the pane's NAME, which \
            never moves. Name a pane you will come back to (open_pane's `name`, or \
            rename_pane) and address it by that."
    });
    let mut roster = json!({
        "tools": [
            {
                "name": "list_panes",
                "description": "List the sibling terminal panes in YOUR window of this sprag \
                    session — NOT every pane the session has; call list_windows for the others. \
                    with their 1-based number, any NAME they have been given (pass a name as \
                    `pane` in place of the number — it does not shift when a pane closes, and a \
                    number does), host id, size, running command, live \
                    window title, and the most recent attention notification a pane \
                    raised (OSC 9 / 777 / 99), if any. Also reports each pane's invisible \
                    input-mode state: whether its app is tracking the MOUSE (DECSET \
                    1000/1002/1003 — clicks, drag, motion) and whether it is tracking \
                    FOCUS (DECSET 1004 — focus in/out), which the pane's on-screen text \
                    does not show and tmux does not expose. Call this first to learn which \
                    number is which pane.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "list_windows",
                "description": "List the WINDOWS of this sprag session — the containers the panes \
                    live in (tmux calls them windows; other terminals call them tabs) — in the \
                    order the user arranged them, marking which one is CURRENT and which one YOU \
                    are in, with each window's panes and the NAMES any of them carry. \
                    `list_panes` answers about ONE window (yours); this is what tells you there \
                    are others at all. Call it when `wait_for_change` reports a \
                    `window_created` / `window_closed` / `window_selected` / `window_renamed` / \
                    `windows_reordered`, and when a pane you are looking for is not among your \
                    siblings: a pane NAME reaches ANY window of this session, so once you know a \
                    name from here you can read_pane, find_in_pane or write_pane it directly. A \
                    pane NUMBER cannot — a number means the Nth pane of YOUR window and nothing \
                    else.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "list_sessions",
                "description": "List this daemon's SESSIONS — the outermost container, one per \
                    workspace a person is keeping — with each one's window and pane counts and \
                    which is the default. Call it when `wait_for_change` reports a \
                    `session_created` / `session_closed` / `session_renamed`, which are changes \
                    you are told about and could otherwise read nothing about. These tools act on \
                    YOUR session only: this answers what else exists, not a way to reach into it.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
            },
            {
                "name": "pane_layout",
                "description": "Draw WHERE the panes sit — the window's arrangement as a tree of \
                    divisions, which pane (if any) is zoomed to fill the window, which panes are \
                    floated out of the tiling, and WHICH PANE IS NEXT TO WHICH in each direction. \
                    `list_panes` answers WHO is in YOUR WINDOW; this answers WHERE they sit. With \
                    no argument it draws YOUR window; name a pane (by NAME) in ANOTHER window and \
                    it draws THAT window instead, so a pane you learned about from `list_windows` \
                    is one call from its arrangement. This answers WHERE, so it is what \
                    to call before choosing a pane by position (\"the pane to the right of mine\", \
                    \"the one below\"). It also marks the pane YOU are running in, which is what \
                    makes a direction mean anything. The neighbour table is the daemon's own \
                    adjacency — the same answer the user's own directional keybinding moves by — \
                    so you never have to work it out from the shape. Which pane the USER is \
                    currently typing into is NOT here: that changes on a keystroke and belongs to \
                    `list_panes`, which marks it.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg.clone() },
                    "additionalProperties": false
                }
            },
            {
                "name": "pane_processes",
                "description": "Say WHAT IS RUNNING in each pane right now — the job that owns \
                    the pane's terminal, with every process in it, its arguments, and the pane's \
                    terminal device. `list_panes` reports the command a pane was SPAWNED with, \
                    which is frozen at its birth: a pane opened as a shell and now running a long \
                    build still lists as that shell. This is the operating system's own answer, so \
                    it is how to tell a pane that is busy from one sitting at a prompt WITHOUT \
                    guessing from its text — a silent build and an idle prompt look identical on \
                    screen, and output that mattered may have scrolled away. Every answer says how \
                    many milliseconds ago it was sampled.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "additionalProperties": false
                }
            },
            {
                "name": "pane_resources",
                "description": "Say WHAT EACH PANE IS TAKING of the machine — the CPU cores it is \
                    holding, how much of the recent past it spent WAITING for cores it did not \
                    get, its memory, and how many processes it holds. `pane_processes` says what \
                    is running; this says what that is costing. Read the two numbers together: \
                    holding little CPU while waiting a lot means this pane is being starved by \
                    another one, and holding little while waiting for nothing means it simply has \
                    nothing to do. If your own work feels slow, this is how to tell which of those \
                    is happening before you change anything. A pane with no rate yet has been \
                    sampled once; ask again in a moment. Every answer says how many milliseconds \
                    ago it was sampled and what window each rate covers.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg.clone() },
                    "additionalProperties": false
                }
            },
            {
                "name": "grant_pane",
                "description": "Say what ONE pane is ALLOWED of the machine — its CPU weight \
                    among its siblings, its memory ceiling and its process ceiling. \
                    `pane_resources` says what a pane TOOK; this is the other half. Reach for it \
                    when that reading shows YOUR pane taking the machine while others wait: hold \
                    your own work back rather than asking a person to. IT ACTS ONLY ON A PANE YOU \
                    OPENED — a person's pane is refused, because how much of their own machine \
                    their work may use is theirs to decide. When the greedy pane is theirs, \
                    report what you measured instead. Every setting is \
                    optional and an omitted one is LEFT ALONE, so you can change a ceiling \
                    without disturbing a weight somebody set earlier; `0` on either ceiling \
                    removes it. \
                    A SHARE IS A WEIGHT, NOT A CAP AND NOT A RATIO. A pane weighted 10 beside an \
                    idle neighbour still takes the whole machine, and a nominal 10:100 split was \
                    measured at 18:82 — so never predict a pane's share from this number, and \
                    read `pane_resources` afterwards for what actually happened. Lowering a \
                    weight shows up in the TAIL: one measurement moved a victim's p99 from 33.3 \
                    ms to 5.4 ms while its median did not move at all. \
                    The answer is RE-READ FROM THE KERNEL, not echoed back, so a ceiling this \
                    host cannot hold comes back saying so instead of looking applied. This is a \
                    real change to the machine and not a reading: what you set stays set for as \
                    long as the pane lives, so lower your weight while a person is waiting and \
                    raise it back when they are not.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg.clone(),
                        "share": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 10000,
                            "description": "CPU weight among sibling panes, 1..=10000. The \
                                default every pane is born with is 100, so 10 is 'let the \
                                others go first' and 1000 is 'prefer this one'."
                        },
                        "memory": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Memory ceiling in MiB; 0 removes it. The pane is \
                                throttled and reclaimed from at this level, never OOM-killed, so \
                                a build that overshoots gets slow rather than dying."
                        },
                        "processes": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Most live processes this pane may hold; 0 removes \
                                the ceiling. Bounds one pane's fork storm from taking the pid \
                                budget the other panes need."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "machine_health",
                "description": "Say WHAT IS WRONG with the machine every pane runs on — not with \
                    sprag. `pane_resources` says which pane is taking the machine; this says why \
                    the machine has less to give. It runs a fixed set of checks and reports EVERY \
                    one of them with the value it measured: whether each pane really has a cgroup \
                    of its own, which resources can be arbitrated between panes at all, whether \
                    something outside this terminal is taking CPU at equal or better weight, \
                    whether the machine is stalled on CPU, disk or memory, whether the panes' \
                    pages have been swapped out, whether more work is runnable than there are \
                    cores, whether a compiler cache is installed and being walked past, and \
                    whether a fast linker is reachable. Use it when work has become slow and \
                    `pane_resources` shows every pane starved rather than one pane greedy — that \
                    is the shape of a machine problem rather than a neighbour problem. A check \
                    that could not read its source says so instead of reporting healthy, so a \
                    clean row means it was looked at. It DETECTS ONLY: each degraded row names \
                    what a person could do and nothing is applied. It costs about half a second, \
                    because one check has to measure a window rather than take a snapshot.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "read_pane",
                "description": "Read a pane's current on-screen text plus scrollback \
                    history (what a human sees in that pane).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "tail_lines": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "If set, return only the last N non-empty-trimmed lines."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "read_last_command",
                "description": "Read just the LAST command a pane's shell ran — its \
                    command line, its output, and its exit status — sliced at the \
                    shell's OSC 133 prompt marks, not the whole screen. Prefer this over \
                    read_pane when you only need the most recent command's result (e.g. \
                    'did that build pass?'). Reports if the command is still running, \
                    and falls back with a note if the pane's shell has no OSC 133 \
                    integration.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "read_pane_links",
                "description": "List the OSC-8 hyperlinks visible in a pane — each link's \
                    displayed text and the URI it points at (https / file / mailto). Use \
                    this to read a link's DESTINATION as data, without OCR or guessing from \
                    the text: `ls --hyperlink`, compiler diagnostics, and doc tools attach \
                    real URIs to text that looks plain on screen. Reports nothing when the \
                    pane shows no links.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "read_pane_images",
                "description": "List the inline images (Kitty graphics / Sixel) a pane is \
                    displaying — each image's id, pixel size, and the cell it is anchored at. \
                    You cannot read an image's contents, but this tells you an image IS \
                    present and where, which the pane's text alone does not. Reports nothing \
                    when the pane shows no images.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "find_in_pane",
                "description": "Search a pane's whole retained output — scrollback AND the \
                    visible screen — for a literal string, and get back the matching lines with \
                    their line numbers. Prefer this over read_pane when you are looking for \
                    something specific ('where did it print the error?'): it searches history \
                    the screen no longer shows and returns only what matched, instead of the \
                    whole buffer for you to scan. Matching is case-insensitive for ASCII and \
                    never spans a line break. Line numbers count from the OLDEST retained line, \
                    so they are stable to quote back.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "needle": {
                            "type": "string",
                            "minLength": 1,
                            "description": "The literal text to search for."
                        }
                    },
                    "required": ["pane", "needle"],
                    "additionalProperties": false
                }
            },
            {
                "name": "regex_in_pane",
                "description": "Like find_in_pane, but the search text is a REGULAR EXPRESSION \
                    (Rust regex syntax) instead of literal text. Use this when what you are \
                    looking for is a shape rather than a string — an error code pattern, one of \
                    several alternatives, a line anchored at its start. It is a separate tool \
                    rather than a flag because the two read the same string differently: 'a.b' \
                    means three literal characters to find_in_pane and 'a, any character, b' \
                    here, so picking the tool is picking the language. Matching is \
                    case-SENSITIVE (find_in_pane folds ASCII case); write (?i) at the start of \
                    the pattern to fold. Anchors ^ and $ bind to each line, and a match never \
                    spans a line break. An invalid pattern is reported with the reason.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "pattern": {
                            "type": "string",
                            "minLength": 1,
                            "description": "The regular expression to search for."
                        }
                    },
                    "required": ["pane", "pattern"],
                    "additionalProperties": false
                }
            },
            {
                "name": "wait_for_output",
                "description": "BLOCK until a pane's output contains what you name, then return the \
                    matching lines. This is 'start the build over there and tell me when it says \
                    DONE' in ONE call — use it instead of calling find_in_pane or read_pane in a \
                    loop. It costs nothing while the pane is quiet and returns as soon as the pane \
                    itself produces the match; there is no polling interval to lose time to. \
                    It searches what the pane has KEPT (its scrollback as well as the visible \
                    screen), so a line that was printed and then scrolled away while you were \
                    waiting still matches — you cannot miss it by looking too late. This is the \
                    right tool when you know the TEXT you are waiting for. If instead you want to \
                    know when a COMMAND finishes (whatever it prints), use wait_for_change with \
                    kinds ['pane_job_changed','pane_closed']; if you want to know when an AGENT in \
                    another pane stops, use wait_for_change with 'pane_agent_state_changed'. \
                    Returns without the match, and WITHOUT failing, if the timeout expires — that \
                    means it has not happened yet, so call again or read the pane to see what it is \
                    actually doing. An invalid `pattern` is reported as an error with the reason, \
                    which is different from 'no match yet'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "needle": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Literal text to wait for, ASCII case-insensitive. Give \
                                this OR `pattern`, never both."
                        },
                        "pattern": {
                            "type": "string",
                            "minLength": 1,
                            "description": "A REGULAR EXPRESSION to wait for (Rust regex syntax), \
                                case-SENSITIVE — write (?i) to fold. Give this OR `needle`, never \
                                both; they are different languages, so 'a.b' means three literal \
                                characters as a needle and 'a, any character, b' as a pattern."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 600,
                            "description": "Give up and report that it has not happened after this \
                                long (default 60). A timeout is not an error."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "agent_state",
                "description": "Report what the AI AGENT in each sibling pane is doing — which one \
                    is waiting for a human, which is still working, and which pane holds no agent at \
                    all. Use this to answer 'is the agent in pane 2 done?' or 'which pane needs \
                    attention?' without reading and interpreting screens. The states are: \
                    `working` (the agent is running: thinking, calling a tool, printing), `blocked` \
                    (it has ASKED something and cannot continue until a human answers — a \
                    permission or trust dialog), and `idle` (at rest, waiting for input it has not \
                    asked for). A pane with NO agent state is reported as such and is NOT idle: \
                    'this is not an agent' and 'this agent is waiting' are opposite facts. Given a \
                    `pane`, reports that one; with no argument, every pane.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "1-based pane number as shown by list_panes. Omit to \
                                report every pane."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "wait_for_change",
                "description": "BLOCK until something changes in this terminal, then report what \
                    changed. Use this instead of polling list_panes, agent_state or pane_processes \
                    in a loop: it costs nothing while nothing is happening and returns the moment \
                    it does. This is the tool for 'wait until the agent in pane 2 finishes', 'wait \
                    until the build in pane 2 finishes', or coordinating several agents. \
                    NARROW IT with `pane` and/or `kinds` and the daemon will not wake you for \
                    anything else — waiting on pane 2 means pane 5's build does not return this \
                    call. Reports typed changes — `pane_agent_state_changed` (an agent started \
                    working, became blocked, or went idle), `pane_job_changed` (the COMMAND \
                    running in a pane changed: the user or an agent started something, or the \
                    thing that was running ended — this is also how a pane's program EXITING is \
                    reported, since a dead pane keeps its place and so is never `pane_closed`), \
                    `pane_created`, `pane_closed`, `pane_renamed` (a pane was given a name, given \
                    a different one, or had it taken away — so an address you were holding may \
                    have stopped resolving), `pane_selected`, `pane_moved` (a pane LEFT one window \
                    for another — it did not die and was not re-created, and the `window` key names \
                    where it went), `window_created`, `window_closed`, `window_selected`, \
                    `window_renamed`, `session_created`, `session_closed`, `session_renamed` (the \
                    session ITSELF was renamed: the `session` key is the name it had — the one you \
                    were holding — and `name` is what it answers to now), `layout_updated`, \
                    `windows_reordered` (a window changed PLACE in the session's order — the \
                    order `list_windows` lists them in, and the one the window keys walk; it \
                    names no window because a swap of two has two equally true readings, so \
                    re-read `list_windows`), `run_finished` (a bounded loop you started with \
                    `orchestrate` reached a terminal state — converged, exhausted a guardrail, \
                    failed, or was cancelled; the `run` key is its id. THIS IS HOW TO WAIT FOR \
                    YOUR OWN LOOP: waiting costs you one call, and polling `list_runs` costs you \
                    one per look, which is the expense `orchestrate` exists to save) — each \
                    naming its SUBJECT, not its new value, except the three that MOVE AN ADDRESS: a \
                    rename and a pane's move also carry the one fact no later read could recover. \
                    Follow up with agent_state, pane_processes or list_panes to read the subject a \
                    change names. To \
                    wait for a command to finish: call this with pane N and kinds \
                    ['pane_job_changed','pane_closed'], then read pane_processes to see what is \
                    running there now (back at the shell means the command is done); the second \
                    kind is there because a pane that dies is the other way the wait can end. \
                    `pane_job_changed` is SAMPLED, so it can arrive up to about 5 seconds after \
                    the fact; every other change above is immediate. Returns immediately if a \
                    change you asked about has already happened since the last call. Pane OUTPUT \
                    is not a change here — it will NOT return this call — read the pane for that.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 600,
                            "description": "Give up and report nothing changed after this long \
                                (default 60). A timeout is not an error."
                        },
                        "pane": {
                            "type": ["integer", "string"],
                            "minimum": 1,
                            "description": "Only wake for changes about THIS pane — a NUMBER (1 = \
                                the first pane in list_panes) or the pane's NAME. Prefer the name \
                                for a long wait: a number can come to mean a different pane if an \
                                earlier one closes while you are parked. Omit to hear about every \
                                pane."
                        },
                        "kinds": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Only wake for these kinds of change, named exactly as \
                                the report names them (e.g. ['pane_job_changed','pane_closed']). \
                                Omit to hear about every kind. An unknown name is refused with the \
                                full list."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "agent_explain",
                "description": "Explain WHY a pane's agent state is what it is: which detection \
                    rule fired, which agent manifest claimed the pane, and how to correct it. Use \
                    this when a state looks wrong — the answer names the rule id, which is the id \
                    you disable or redefine in an `[[agent]]` block in sprag's config.toml. The \
                    rule is read from the verdict the detector already produced, so it can never \
                    disagree with what agent_state reports. A pane no manifest claims is explained \
                    as exactly that, which is the diagnosable answer for 'why does my agent pane \
                    show nothing' — and if that config.toml will not parse at all, the answer says \
                    so first, because an unparsed claim is indistinguishable from an absent one.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "write_pane",
                "description": "Type literal text into a sibling pane's shell, as if the \
                    user typed it, and (by default) press Enter to run it. Use this to \
                    run a command in another pane.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "text": { "type": "string", "description": "The literal UTF-8 text to type." },
                        "enter": {
                            "type": "boolean",
                            "description": "Press Enter after the text (default true)."
                        }
                    },
                    "required": ["pane", "text"],
                    "additionalProperties": false
                }
            },
            {
                "name": "send_keys",
                "description": "Send one or more named keys to a pane (W3C key names: \
                    \"Enter\", \"Escape\", \"Tab\", \"ArrowUp\", \"Backspace\", or a single \
                    character like \"c\"). Combine with ctrl/alt/shift for chords such as \
                    Ctrl+C (keys=[\"c\"], ctrl=true).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "W3C key names to send in order."
                        },
                        "ctrl": { "type": "boolean" },
                        "alt": { "type": "boolean" },
                        "shift": { "type": "boolean" },
                        "super": { "type": "boolean" }
                    },
                    "required": ["pane", "keys"],
                    "additionalProperties": false
                }
            },
            {
                "name": "open_pane",
                "description": "Open a NEW pane in this terminal to do your own work in — a \
                    place to run a build, a test suite or a long command WITHOUT taking over \
                    the pane a person is reading. It runs a shell, so type commands into it \
                    with write_pane and read them back with read_pane; the output stays there \
                    after the command finishes. Every other tool here works on panes somebody \
                    else opened; this is how you make your own. The pane is recorded as opened \
                    BY YOUR PANE, which is what lets close_pane let you clean it up later (and \
                    what stops you closing a person's). GIVE IT A NAME: a pane number is \
                    positional and shifts when an earlier pane closes, so a name is the only \
                    handle you can still trust later — every tool here takes it wherever it \
                    takes a number. The answer re-lists every pane with its number. It does NOT \
                    move the user's cursor — call select_pane if you want them to look at it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "What to call the pane, e.g. \"build\" or \"tests\". \
                                Use it in place of a number in any tool's `pane` argument. Must \
                                be unique in this terminal, at most 80 bytes, and not all \
                                digits (a number means the position)."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Directory the new shell starts in. Defaults to \
                                this server's own working directory."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "close_pane",
                "description": "Close a pane YOU opened with open_pane, ending what runs in it \
                    and discarding its scrollback. A pane you did not open is refused — a \
                    person's pane may hold unsaved work, and a mis-typed pane number must not \
                    destroy it. Read anything you still need with read_pane FIRST: closing is \
                    not undoable. The answer re-lists every pane, because closing one RENUMBERS \
                    the panes after it.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "rename_pane",
                "description": "Give a pane YOU opened a NAME, or change the one it has. A name \
                    is a stable handle: pass it as `pane` to any tool here instead of the \
                    number, which is positional and shifts whenever an earlier pane closes. Use \
                    it when a pane's job changes (\"build\" becomes \"tests\") — you can \
                    normally name a pane when you open it. A pane you did not open is refused: \
                    its name is what a PERSON sees on it, and renaming somebody's pane is not \
                    yours to do. Names are unique in this terminal, at most 80 bytes, and never \
                    all digits (a number means the position).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "name": {
                            "type": "string",
                            "description": "The new name. Omit it to take the pane's name away."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "display_message",
                "description": "Say something to the PERSON at this terminal — one line on the \
                    status line of every window attached to it. Use it when you have finished \
                    something they are waiting on, or when you are blocked and need them: a long \
                    build that failed, a question you cannot answer yourself, a deploy that wants \
                    a decision. It is the ONLY way to reach somebody who is looking at a different \
                    pane; send_keys types into their program instead of telling them anything, and \
                    a line you print in your own pane is invisible to a person working elsewhere. \
                    DO NOT narrate with it: one message when something needs them, not progress \
                    reports. It is one line — under 200 bytes, no newlines or escape codes — and \
                    the terminal refuses anything else rather than quietly cutting it. \
                    `severity` decides what happens if they are not looking: `note` (the default) \
                    and `warn` go away by themselves after a moment, so a person away from the \
                    keyboard misses them; `alert` stays on the row until they press a key, and a \
                    lower severity can never wipe a live one. Reserve `alert` for things that are \
                    useless if missed. THE ANSWER TELLS YOU WHO SAW IT: if no window is attached \
                    it says so, and you must not treat the message as delivered — leave the \
                    evidence somewhere durable as well.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "The one line to show. Write it for somebody who has \
                                not been watching: say what happened and what it needs, not \
                                'done'."
                        },
                        "severity": {
                            "type": "string",
                            "enum": Severity::ALL.map(Severity::word),
                            "description": "How much it matters. `note` (default) and `warn` \
                                expire on their own; `alert` stays until the person presses a key."
                        }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }
            },
            {
                "name": "select_pane",
                "description": "Make a pane the ACTIVE one for this terminal session — where \
                    the user's keystrokes go, what every attached window shows a focus ring \
                    on, and what a pane command with no target acts on. Use it to put a human \
                    on the pane you want them to look at, after opening or preparing it. \
                    `list_panes` marks the active pane. This MOVES a person's cursor: prefer \
                    it when you have something for them to see, not as a side effect. A pane in \
                    ANOTHER window is the exception and the answer says so: it becomes THAT \
                    window's active pane and the person does not move, because nothing here \
                    changes which window somebody is looking at. \
                    Give EITHER `pane` (that pane) OR `dir` (one step that way through the \
                    arrangement, like a tmux `select-pane -L`) — never both. By default `dir` \
                    steps FROM WHERE THE USER IS NOW; add `from` (a pane number or name) or \
                    `from_here: true` (the pane YOU are running in) to step from a pane you \
                    choose instead. The terminal resolves the step against the live \
                    arrangement in the same moment it moves: reading `pane_layout` yourself \
                    and then selecting a number would ask two questions at two moments and can \
                    land the user on a pane that closed in between. The answer says what \
                    happened, including \"there is nothing that way\" — which is a normal \
                    outcome at the edge of a layout, not a failure.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "dir": {
                            "type": "string",
                            "enum": PaneDir::ALL.map(PaneDir::wire_str),
                            "description": "Move one pane that way. Use pane_layout to see the \
                                arrangement first if you need to know what lies that way."
                        },
                        "from": {
                            "description": "Which pane the `dir` step starts at — a NUMBER \
                                (1-based, see list_panes) or a pane's NAME. Omit it to step \
                                from the pane the user is on. Only with `dir`.",
                            "type": ["integer", "string"]
                        },
                        "from_here": {
                            "type": "boolean",
                            "description": "Step from the pane YOU are running in, without \
                                looking its number up (which could name a different pane by \
                                the time you send it). Only with `dir`, and never together \
                                with `from`."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "swap_pane",
                "description": "Move a pane YOU OPENED to a different place in the arrangement, by \
                    trading places with another pane. Use it to put a pane you opened where the \
                    user can see it beside something — next to the editor, under the one it \
                    belongs with. `pane` is REQUIRED and names the pane being MOVED: only a pane \
                    you opened yourself with open_pane can be moved, because where a person's own \
                    panes sit is their arrangement and not yours. Give EITHER `with` (trade with \
                    that pane) OR `dir` (trade with the pane one step that way, like a tmux \
                    `swap-pane -L`) — never both. Both panes keep their size and their contents; \
                    only their PLACES are exchanged, and nobody's cursor moves — use select_pane \
                    for that. The answer says what happened, including \"there is nothing that \
                    way\", which is a normal outcome at the edge of a layout and not a failure. \
                    Call pane_layout first if you need to know what lies where.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "with": {
                            "description": "The pane to trade places with — a NUMBER (1-based, \
                                see list_panes) or a pane's NAME. It may be any pane, including \
                                one a person opened: it is displaced by the trade, not moved by \
                                a decision of yours.",
                            "type": ["integer", "string"]
                        },
                        "dir": {
                            "type": "string",
                            "enum": PaneDir::ALL.map(PaneDir::wire_str),
                            "description": "Trade with the pane one step that way from `pane`, \
                                resolved against the live arrangement in the same moment it \
                                moves. Use pane_layout to see what lies that way."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "resize_pane",
                "description": "Make a pane YOU OPENED wider or taller by moving the boundary \
                    between it and its neighbour. Use it when the output you are READING is \
                    wrapping: a pane's width is what decides whether a build log, a table or a \
                    stack trace arrives in one line per line, so widening your own pane changes \
                    what read_pane can see. `pane` is REQUIRED and names the pane whose boundary \
                    moves: only a pane you opened yourself with open_pane can be resized, because \
                    how big a person's own panes are is their arrangement and not yours. `dir` \
                    moves the BOUNDARY that way — so \"right\" makes a pane on the LEFT of it \
                    wider and a pane on the RIGHT of it narrower — and `cells` says how far, in \
                    terminal cells, defaulting to 1. The neighbour gives up exactly what your \
                    pane gains; the window does not change size. The answer says how many cells \
                    it ACTUALLY moved, which is fewer than you asked for when it reached the last \
                    cell the far side may keep — a normal outcome, not a failure. Call \
                    pane_layout first if you need to know what lies where.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg,
                        "dir": {
                            "type": "string",
                            "enum": PaneDir::ALL.map(PaneDir::wire_str),
                            "description": "Which way the BOUNDARY moves. Whether `pane` grows \
                                or shrinks follows from which side of that boundary it is on."
                        },
                        "cells": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "How far, in terminal cells. Defaults to 1. A width \
                                is a count of columns, so ask for the columns you need."
                        }
                    },
                    "required": ["pane", "dir"],
                    "additionalProperties": false
                }
            },
            {
                "name": "zoom_pane",
                "description": "Make a pane YOU OPENED fill its whole window, or put the \
                    arrangement back. Use it when what you are READING needs the room — a wide \
                    diff, a table, a log that wraps — because a zoomed pane is given the window's \
                    full width and height, and read_pane sees exactly those columns. `pane` is \
                    REQUIRED and names the pane that fills: only a pane you opened yourself with \
                    open_pane is yours to zoom, because which pane fills a person's window decides \
                    what they can see. With no `on` it TOGGLES, so calling it twice puts things \
                    back; pass `on: false` to un-zoom explicitly. THE PERSON SEES THIS if they are \
                    looking at that window — the other panes are hidden while it lasts, so unzoom \
                    when you are done reading. Zooming also SELECTS the pane it fills. Nothing in \
                    the arrangement is edited: pane_layout still reports where every pane is, and \
                    the panes come back exactly as they were.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg.clone(),
                        "on": {
                            "type": "boolean",
                            "description": "`true` fills the window, `false` puts the arrangement \
                                back. Omit to toggle whichever state the pane is in."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "break_pane",
                "description": "Take a pane YOU OPENED out of the window it is in and give it a \
                    WINDOW OF ITS OWN, without moving the user. Use it when a pane you opened has \
                    grown into real work — a long build, a server you want to keep — and is \
                    crowding the window a person is reading: this gets it out of their way while \
                    keeping everything in it, because the pane is MOVED whole and its scrollback, \
                    its running command and its NAME all ride along. `pane` is REQUIRED and only a \
                    pane you opened is yours to move. The new window is created DETACHED, exactly \
                    like open_window: the user stays where they are and does not see it, and \
                    select_window is how you show them. It is recorded as opened by you, so you \
                    can close_window and rename_window it afterwards. REFUSED if the pane is the \
                    only one its window tiles — that would be a rename dressed as a move; use \
                    rename_window instead.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg.clone(),
                        "name": {
                            "type": "string",
                            "description": "What to call the new window — how you address it \
                                afterwards, and what a person sees in the window list. Omit for \
                                the lowest free number, but a name says whose work it is."
                        }
                    },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            },
            {
                "name": "join_pane",
                "description": "Move a pane YOU OPENED into another WINDOW of this session, where \
                    it is added beside what is already there. The reverse of break_pane, and the \
                    way to gather panes you opened in different places into one window of your \
                    own. `pane` is REQUIRED and only a pane you opened is yours to move; \
                    `window` is any window of the session, named as list_windows names it. Moving \
                    a pane INTO A PERSON'S WINDOW puts it on their screen beside their own panes \
                    — do that when you want them to see it, not to tidy up. The pane is moved \
                    whole: same contents, same scrollback, same name. If the move empties the \
                    window the pane came from, that window CLOSES and the answer says so. Use \
                    move_pane instead when WHERE in the destination it lands matters.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg.clone(),
                        "window": {
                            "type": "string",
                            "description": "The destination window's NAME, from list_windows."
                        }
                    },
                    "required": ["pane", "window"],
                    "additionalProperties": false
                }
            },
            {
                "name": "move_pane",
                "description": "Put a pane YOU OPENED on a particular SIDE of a particular pane — \
                    join_pane with a PLACE. Use it when the pane has to land somewhere specific: \
                    below the pane whose output it belongs with, to the right of the editor. It \
                    works whether the target is in the same window or another one, so it is also \
                    how you move a pane between windows and say where it arrives; join_pane \
                    appends instead, which says where only by convention. `pane` is REQUIRED and \
                    only a pane you opened is yours to move; `target` may be any pane, including a \
                    person's — it is divided to make room, not moved by a decision of yours. `dir` \
                    is the SAME four words every other tool here takes: it says which side of the \
                    target the moved pane lands on, so \"left\" puts it left of the target and \
                    \"down\" puts it below. If the move empties the window the pane came from, \
                    that window CLOSES and the answer says so. Call pane_layout first if you need \
                    to know what is where.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pane": pane_arg.clone(),
                        "target": {
                            "description": "The pane to land beside — a NUMBER (1-based, see \
                                list_panes) or a pane's NAME. A NAME reaches any window of this \
                                session; a number means the Nth pane of YOUR window.",
                            "type": ["integer", "string"]
                        },
                        "dir": {
                            "type": "string",
                            "enum": PaneDir::ALL.map(PaneDir::wire_str),
                            "description": "Which SIDE of the target the moved pane lands on. \
                                The target is divided to make room; nothing else moves."
                        }
                    },
                    "required": ["pane", "target", "dir"],
                    "additionalProperties": false
                }
            },
            {
                "name": "open_window",
                "description": "Open a NEW WINDOW of this session to do your own work in, WITHOUT \
                    moving the user. `open_pane` gives you a pane beside somebody — this gives you \
                    a whole screenful nobody is looking at, which is what to use when your work \
                    would crowd a person's window or when you want several panes of your own. It \
                    is created DETACHED: the user stays exactly where they are and does not see \
                    it. When you have something for them, call `select_window` — showing it is a \
                    separate act on purpose. Name it, and address its panes by THEIR names \
                    afterwards; a window you opened is yours to `close_window` and \
                    `rename_window`, and a person's window is refused.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "What to call the window — how a person will see it in \
                                the window list, and how you address it afterwards. Omit for the \
                                lowest free number, but a name says whose work it is."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Which directory the window's shell starts in — an \
                                absolute path. Defaults to wherever the daemon starts a shell. \
                                `open_pane` takes the same argument and means the same thing."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "select_window",
                "description": "Move the USER to another window of this session — the window-level \
                    twin of `select_pane`, and the only verb here that changes which screenful a \
                    person is looking at. Use it when you have something for them to SEE (a build \
                    you finished in a window you opened, a pane that needs their answer), never as \
                    a side effect: it takes their whole screen, and every attached client follows. \
                    Give `window` (that window, by name — `list_windows` names them) or `relative` \
                    (one step along the ring, which wraps). It answers where they landed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "window": {
                            "type": "string",
                            "description": "The window's NAME, from list_windows."
                        },
                        "relative": {
                            "type": "string",
                            "enum": OrderStep::ALL.map(OrderStep::wire_str),
                            "description": "One step along the window ring instead of naming one. \
                                The ring wraps, so a step always lands somewhere."
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "close_window",
                "description": "Close a window YOU opened, and every pane in it. Refused for a \
                    window a person made — that is their workspace — and refused when it is the \
                    session's LAST window, because closing that would end the person's whole \
                    session and a tidy-up must not do that. Use it when the work you opened a \
                    window for is done.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "window": {
                            "type": "string",
                            "description": "The window's NAME, from list_windows."
                        }
                    },
                    "required": ["window"],
                    "additionalProperties": false
                }
            },
            {
                "name": "rename_window",
                "description": "Change the name of a window YOU opened. Refused for a person's \
                    window: a window's name is what THEY read in the window list. The name is also \
                    the address, so what you rename it to is what you must call it afterwards.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "window": {
                            "type": "string",
                            "description": "The window's CURRENT name, from list_windows."
                        },
                        "name": {
                            "type": "string",
                            "description": "What to call it instead."
                        }
                    },
                    "required": ["window", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "resize_window",
                "description": "Force the cell size of a window YOU opened, or give the forcing \
                    back. Refused for a person's window: how big their window is belongs to them. \
                    Use it when what you are READING needs a shape the people watching cannot \
                    give it — a wide table, a diff, a log that wraps — because a window's size is \
                    what decides the columns every pane in it gets, and read_pane sees exactly \
                    those columns. Name a rectangle with `cols` and `rows` together, or pass \
                    NEITHER to un-pin and let the window follow the clients again. IT MAY DO \
                    NOTHING: a pinned size is only laid out when the `window-size` option is \
                    `manual`, and the answer SAYS which policy is in force, so read it rather \
                    than assuming the panes moved. A window bigger than a person's terminal shows \
                    them only part of it, which is why this is a tool for a window you own.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "window": {
                            "type": "string",
                            "description": "The window's name, from list_windows."
                        },
                        "cols": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "How many columns wide. Give `rows` too, or neither."
                        },
                        "rows": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "How many rows tall. Give `cols` too, or neither."
                        }
                    },
                    "required": ["window"],
                    "additionalProperties": false
                }
            }
        ]
    });
    // ⚠ APPENDED RATHER THAN WRITTEN INSIDE THE LITERAL ABOVE, and not by choice: three more
    // entries put `json!` past its expansion recursion limit. Raising `recursion_limit` would have
    // bought one more round of the same, so the roster splits by SUBJECT instead — which is also
    // where it was going, since these three are the only tools whose schema is DERIVED rather than
    // written.
    if let Some(list) = roster["tools"].as_array_mut() {
        list.extend(orchestration_tools());
    }
    roster
}

/// The three tools that reach the orchestration loop.
///
/// Their schemas come from [`orchestrate_schema`], which reads the wire's own published grammar —
/// so a plugin added upstream is advertised here in the compile that adds it, and no argument name
/// is spelled twice in this workspace.
fn orchestration_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "orchestrate",
            "description": orchestrate_description(),
            "inputSchema": orchestrate_schema()
        }),
        json!({
            "name": "list_runs",
            "description": "List the bounded loops YOU started with orchestrate, and how the \
                finished ones ended — the outcome (converged / exhausted / failed / cancelled), \
                how many iterations it took, what it spent in its own cost unit, and any reply the \
                run captured. Runs another agent or the person started are NOT listed: a run \
                carries the pane that asked for it, and this answers about yours. Poll this after \
                orchestrate rather than watching the pane — a run's outcome is a level, so it is \
                still here whether or not you were looking when it finished.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "cancel_run",
            "description": "Ask one of YOUR runs to stop. It stops at its next step rather than \
                being killed mid-write, so the pane it was driving is left in a state somebody can \
                read. A run you did not start is refused — cancelling somebody else's loop is a \
                decision about their work. Use it when the pane's output shows the loop is not \
                going to converge; you do not need it for safety, because every run is already \
                bounded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "The run id, from orchestrate or list_runs."
                    }
                },
                "required": ["run"],
                "additionalProperties": false
            }
        }),
    ]
}

/// THE ARGUMENTS OF `run` THAT NAME A PANE, and so resolve through this surface's own addressing.
///
/// # Why a list, and why it is safe to be one
///
/// The published grammar says these are `int`; it does not say they are PANES. Nothing in
/// [`sprag_rpc::ArgGrammar`] can, and inventing a subject axis for three keys would be a wire
/// change made for one consumer. So the list is here — and
/// [`every_int_argument_of_a_run_is_classified`](self) makes it fail-closed: every `int` argument
/// of every published form is either here or in [`NOT_A_PANE`], so an argument added upstream
/// forces a decision instead of silently arriving unresolved and unauthorised.
const PANE_ARGUMENTS: &[&str] = &["pane", "src", "dst"];

/// The `int` arguments of `run` that are NOT panes — a count, a size, a bound, a provenance.
///
/// The other half of [`PANE_ARGUMENTS`]'s fail-closed pair. An entry here is a decision that this
/// argument needs no ownership check, which is true of every number that is not a pane.
const NOT_A_PANE: &[&str] = &[
    "timeout_ms",
    "cols",
    "rows",
    "opened_by",
    "max_iterations",
    "max_seconds",
    "max_bytes",
    "max_tokens",
];

/// The argument the SERVER stamps and an agent may never send.
///
/// # ⚠⚠ The authority decision, in one constant
///
/// `opened_by` is what makes a run somebody's. If an agent could set it, it could claim a run as
/// another pane's — or, worse, list and cancel that pane's runs by asserting its number. So it is
/// absent from the advertised schema, refused if sent, and filled in by this server from its OWN
/// pane. That is the same shape as every other tool here: the agent says what it wants done, and
/// the surface says who is asking.
const OPENED_BY: &str = "opened_by";

/// The `run` forms this build was compiled against — the ONE definition the tool schema, the
/// description and the argument classification all read.
fn run_forms() -> &'static [sprag_rpc::CallForm] {
    sprag_host::wire::PLUGINS_GRAMMAR
        .iter()
        .find(|verb| verb.action == sprag_host::plugins::RUN_ACTION)
        .map_or(&[], |verb| verb.forms)
}

/// ONE ARGUMENT of `orchestrate`, MERGED across the forms that declare it.
///
/// A form-level fact folded into a flat one, and the merge is not a formality: the `plugin`
/// discriminator publishes exactly ONE word per form (that is how a form is selected), so a schema
/// that took the first occurrence would advertise `orchestrator` as the only plugin an agent may
/// name. The union across forms is the whole vocabulary — which is what
/// [`PluginGrammar`](sprag_host::wire::PluginGrammar) says the union is for.
struct RunArgument {
    name: &'static str,
    ty: &'static str,
    /// Every word any form admits for it, or empty for an open value.
    words: Vec<&'static str>,
}

/// Every argument of every `run` form, nesting flattened, minus the one the agent may not send.
///
/// Merged by NAME across forms: an MCP `inputSchema` is one flat object and the wire's four forms
/// are an alternation, which JSON Schema could express with `oneOf` and which no MCP client is
/// guaranteed to enforce. The description carries the alternation instead (see
/// [`orchestrate_description`]) and the DAEMON is what actually refuses a mixed call — which is the
/// right place for it, since the daemon is the thing that knows.
fn orchestrate_arguments() -> Vec<RunArgument> {
    let mut out: Vec<RunArgument> = Vec::new();
    for form in run_forms() {
        for arg in form.args {
            for arg in std::iter::once(arg).chain(arg.fields) {
                if arg.name == OPENED_BY || !arg.fields.is_empty() {
                    continue;
                }
                let words = arg.words.unwrap_or_default();
                match out.iter_mut().find(|seen| seen.name == arg.name) {
                    Some(seen) => {
                        for word in words {
                            if !seen.words.contains(word) {
                                seen.words.push(word);
                            }
                        }
                    }
                    None => out.push(RunArgument {
                        name: arg.name,
                        ty: arg.ty,
                        words: words.to_vec(),
                    }),
                }
            }
        }
    }
    out
}

/// The `orchestrate` tool's input schema, DERIVED from the wire's published grammar.
///
/// Not hand-written, for this crate's standing reason: a roster that re-spells the daemon's
/// arguments is a second list, and a second list is the one a new plugin is left out of. A plugin
/// added to `PluginName` reaches this schema in the compile that adds it.
fn orchestrate_schema() -> Value {
    let mut properties = serde_json::Map::new();
    for arg in orchestrate_arguments() {
        let schema = if PANE_ARGUMENTS.contains(&arg.name) {
            json!({
                "type": ["integer", "string"],
                "minimum": 1,
                "description": "Which pane — a NUMBER from list_panes or a NAME, exactly as every \
                    other tool here takes one. It must be a pane YOU opened."
            })
        } else {
            let mut schema = serde_json::Map::new();
            schema.insert(
                "type".to_owned(),
                match arg.ty {
                    "int" => json!("integer"),
                    "bool" => json!("boolean"),
                    "array" => json!("array"),
                    "object" => json!("object"),
                    _ => json!("string"),
                },
            );
            if arg.ty == "array" {
                schema.insert("items".to_owned(), json!({ "type": "string" }));
            }
            if !arg.words.is_empty() {
                schema.insert("enum".to_owned(), json!(arg.words));
            }
            schema.insert("description".to_owned(), json!(argument_help(arg.name)));
            Value::Object(schema)
        };
        properties.insert(arg.name.to_owned(), schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        // ONLY the discriminator, because the other required arguments differ per plugin and a
        // flat `required` would demand a dialogue's `seed` of an agent run. The daemon refuses a
        // missing one by name.
        "required": ["plugin"],
        "additionalProperties": false,
    })
}

/// What one argument of `orchestrate` is FOR, in an agent's terms.
///
/// A per-name table and not a rule, because a type cannot say what a stimulus is. It is keyed by
/// the published name so an argument this build does not know still appears in the schema with an
/// honest blank rather than being dropped.
fn argument_help(name: &str) -> &'static str {
    match name {
        "plugin" => {
            "Which loop to run. `agent` prompts the AI in a pane and collects its reply; \
             `orchestrator` retypes one stimulus until a sentinel appears; `pipe` relays one \
             pane's output into another's input; `dialogue` runs two commands against each other, \
             turn by turn."
        }
        "stimulus" => "The text typed into the pane each iteration (orchestrator).",
        "sentinel" => {
            "Stop as soon as this appears on the pane (orchestrator). Without it the \
             run goes to its iteration ceiling."
        }
        "prompt" => "What to say to the agent in the pane (agent).",
        "eof" => "Send end-of-input after the prompt (agent), for a command that reads until EOF.",
        "timeout_ms" => "How long one turn may take before the run gives up on it.",
        "seed" => "The first message, given to endpoint A (dialogue).",
        "endpoint_a" => "The command line of the first speaker, as a list (dialogue).",
        "endpoint_b" => "The command line of the second speaker, as a list (dialogue).",
        "label_a" => "What to call the first speaker in the transcript (dialogue).",
        "label_b" => "What to call the second speaker in the transcript (dialogue).",
        "format_a" => "How to read the first speaker's reply (dialogue).",
        "format_b" => "How to read the second speaker's reply (dialogue).",
        "cols" => "How wide the panes a dialogue spawns are.",
        "rows" => "How tall the panes a dialogue spawns are.",
        "max_iterations" => {
            "Stop after this many turns. It may only be LOWER than this daemon's \
             default, never higher — the ceiling is the person's to raise."
        }
        "max_seconds" => {
            "Stop after this many seconds of wall-clock time, whatever the run is doing — the \
             bound to set when what you care about is not spending the afternoon on it. It is \
             separate from timeout_ms, which bounds ONE turn and then lets the loop take another. \
             Lower than the default only."
        }
        "max_bytes" => "Stop after injecting this many bytes. Lower than the default only.",
        "max_tokens" => "Stop after this many model tokens. Lower than the default only.",
        _ => "See `sprag show-grammar run` for what this daemon says about this argument.",
    }
}

/// The `orchestrate` tool's description, with one line per form — the alternation the flat schema
/// above cannot carry, written from the same table.
fn orchestrate_description() -> String {
    let mut text = String::from(
        "Run a BOUNDED loop against panes and get a run id back immediately. This is what you \
         should use instead of hand-rolling a drive-and-wait loop in your own turns: the platform \
         enforces an iteration ceiling, a cost ceiling in the run's own unit, a wall-clock \
         deadline, and a cancel flag, \
         and it ends a turn on the agent's MEASURED state rather than on a timer. Your loop has \
         none of that. It returns at once — poll list_runs for the outcome. Every pane it touches \
         must be one YOU opened. Forms:",
    );
    for form in run_forms() {
        let Some(word) = form
            .args
            .iter()
            .find(|arg| arg.words.is_some_and(|words| words.len() == 1))
            .and_then(|arg| arg.words.and_then(<[&str]>::first))
        else {
            continue;
        };
        let needed: Vec<&str> = form
            .args
            .iter()
            .filter(|arg| !arg.optional && arg.name != "plugin")
            .map(|arg| arg.name)
            .collect();
        text.push_str(&format!("\n  {word}: needs {}", needed.join(", ")));
    }
    text
}

/// `orchestrate`: start a bounded loop for the agent that asked, on the panes it owns.
///
/// # The three things this adds to the wire's own `run`, and why each is the MOUTH's job
///
/// The daemon accepts a `run` from anyone for any pane with any bound, and that is right: it has no
/// authentication, and a person driving their own machine should not be second-guessed. This
/// surface is the one with a caller it can identify, so it is the one that can say:
///
/// 1. **Every pane must be the agent's own** — [`require_own_pane`], the rule the five other
///    writing tools keep. Without it a plugin run would be a laundering path around them: an agent
///    refused `write_pane` on a person's pane could have driven the same pane through a loop.
/// 2. **A guardrail may only TIGHTEN** — the daemon's published defaults are the ceiling, and an
///    agent asking for more is REFUSED rather than silently clamped. A run that quietly did
///    something other than what it was asked is how a guardrail becomes folklore.
/// 3. **The run carries who asked** — stamped here from this server's own pane, never taken from
///    the caller ([`OPENED_BY`]), which is what makes `list_runs` and `cancel_run` answer about the
///    caller's own work.
fn tool_orchestrate(args: &Value) -> Result<String, String> {
    let mut action_args = serde_json::Map::new();
    let known = orchestrate_arguments();
    let object = args.as_object().cloned().unwrap_or_default();

    if object.contains_key(OPENED_BY) {
        return Err(format!(
            "'{OPENED_BY}' is not yours to set — this server stamps the run with the pane you are \
             running in, which is what makes list_runs and cancel_run answer about your own runs."
        ));
    }
    for key in object.keys() {
        if !known.iter().any(|arg| arg.name == key) {
            return Err(format!(
                "'{key}' is not an argument of orchestrate. It takes: {}",
                known
                    .iter()
                    .map(|arg| arg.name)
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }

    let ceilings = guardrail_defaults()?;
    let mut guardrails = serde_json::Map::new();
    for arg in &known {
        let Some(value) = object.get(arg.name) else {
            continue;
        };
        // A PANE argument resolves through this surface's own addressing and must be the agent's.
        if PANE_ARGUMENTS.contains(&arg.name) {
            let pane = resolve_pane_ref_at(args, arg.name)?;
            require_own_pane(
                &pane,
                "orchestrate",
                "A loop drives a pane exactly as write_pane and send_keys do, so it is refused for \
                 the same reason: open your own pane with open_pane and orchestrate that. If the \
                 work has to happen in the person's pane, tell them what you would run.",
            )?;
            action_args.insert(arg.name.to_owned(), json!(pane.id()));
            continue;
        }
        // A GUARDRAIL field is checked against this daemon's own published default and then moves
        // inside the nested object the wire takes.
        if let Some(ceiling) = ceilings.get(arg.name) {
            let asked = value
                .as_u64()
                .ok_or_else(|| format!("'{}' must be a whole number", arg.name))?;
            if asked > *ceiling {
                return Err(format!(
                    "'{}' may be at most {ceiling} here, and {asked} was asked for. A guardrail an \
                     agent could raise is not a guardrail — lower it, or ask the person to raise \
                     this daemon's default.",
                    arg.name,
                ));
            }
            guardrails.insert(arg.name.to_owned(), json!(asked));
            continue;
        }
        // ⚠⚠ FAIL CLOSED ON AN UNCLASSIFIED NUMBER. Every `int` this daemon publishes is either a
        // pane (resolved and ownership-checked above) or one of [`NOT_A_PANE`]. A number that is
        // neither is one this build cannot tell apart from a pane id — so passing it through would
        // be handing the wire a pane reference that skipped the ownership rule, which is the one
        // thing this surface exists to apply.
        //
        // ⚠ UNREACHABLE AGAINST A DAEMON OF THIS BUILD, and that is the point rather than a gap:
        // `every_int_argument_of_a_run_is_classified` proves the two lists cover every `int` the
        // compiled-in grammar publishes, so nothing this workspace serves can get here. The branch
        // is for a daemon that is NEWER than this binary — the one case where the classification
        // cannot have been made in advance, and the one where guessing would be worst.
        if arg.ty == "int" && !NOT_A_PANE.contains(&arg.name) {
            return Err(format!(
                "this daemon's '{}' is a number that this server does not know how to check — it \
                 may name a pane, and a pane argument has to be one you own. Start the run without \
                 it, or use `sprag orchestrate` where the person is the authority.",
                arg.name,
            ));
        }
        action_args.insert(arg.name.to_owned(), value.clone());
    }
    if !guardrails.is_empty() {
        action_args.insert("guardrails".to_owned(), Value::Object(guardrails));
    }
    // ⚠ BEFORE the invoke, never after: a run submitted first can finish first, and an anchor taken
    // afterwards would sit past its own `run_finished` record — the exact race this exists to close.
    anchor_change_cursor();
    // WHO ASKED — this server's own pane, never the caller's word for it.
    let mine = own_pane().ok_or_else(|| {
        "orchestrate needs to know which pane you are in, and this process is not inside one — so \
         a run started here could not be told apart from anybody else's."
            .to_owned()
    })?;
    action_args.insert(OPENED_BY.to_owned(), json!(mine));

    let answer = host_call(
        "scene/invoke",
        json!({
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": Value::Object(action_args),
        }),
    )?;
    let id = answer
        .as_u64()
        .ok_or_else(|| "the daemon's answer was not a run id".to_owned())?;
    Ok(format!(
        "Run {id} started. It is bounded: it stops at its iteration ceiling, its cost ceiling or \
         its wall-clock deadline, whichever binds first — and when it stops it says WHICH of them \
         it was. Call list_runs for the outcome — it is still there when you look, even if the run \
         finished while you were doing something else.\n"
    ))
}

/// This daemon's guardrail defaults, which are this surface's CEILINGS.
///
/// Read from the daemon rather than from [`sprag_host::plugins::DEFAULT_MAX_ITERATIONS`] and its
/// siblings, though this binary could see both: the ceiling an agent is held to must be the one the
/// daemon actually enforces, and a constant compiled into a separately-built client is a different
/// number the day the two are not the same build. That is `show-grammar`'s whole argument, applied
/// to the one fact that decides whether a bound is real.
fn guardrail_defaults() -> Result<std::collections::BTreeMap<String, u64>, String> {
    let answer = host_call(
        "scene/query",
        json!({
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::GUARDRAIL_DEFAULTS_SLOT),
        }),
    )?;
    let map = answer
        .as_object()
        .ok_or_else(|| "the daemon's guardrail defaults were not an object".to_owned())?;
    Ok(map
        .iter()
        .filter_map(|(key, value)| Some((key.clone(), value.as_u64()?)))
        .collect())
}

/// `list_runs`: the runs THIS agent started, with where each got to.
fn tool_list_runs() -> Result<String, String> {
    let mine = own_runs()?;
    if mine.is_empty() {
        return Ok("You have started no runs. orchestrate starts one.\n".to_owned());
    }
    let mut out = String::new();
    for run in mine {
        out.push_str(&render_run(&run));
    }
    Ok(out)
}

/// Every run this agent's own pane asked for.
///
/// ⚠ The registry is DAEMON-WIDE and the filter is what makes this surface's promise true: the
/// `runs` slot answers with every run the host holds, including a person's and another agent's, so
/// an unfiltered tool would report on work its caller cannot see and did not start. The provenance
/// it filters by is the one the daemon recorded at submit time.
fn own_runs() -> Result<Vec<Value>, String> {
    let mine = own_pane().ok_or_else(|| {
        "this process is not inside a pane, so there is no way to say which runs are yours."
            .to_owned()
    })?;
    let answer = host_call(
        "scene/query",
        json!({ "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT) }),
    )?;
    Ok(answer
        .as_array()
        .map(|runs| {
            runs.iter()
                .filter(|run| run["opened_by"].as_u64() == Some(mine))
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

/// One run as an agent reads it.
fn render_run(run: &Value) -> String {
    let id = run["id"].as_u64().unwrap_or_default();
    let label = run["label"].as_str().unwrap_or("?");
    let state = &run["state"];
    match state["status"].as_str() {
        // The counters, for the reason the person's renderer prints them: an agent that polls a
        // long run and sees the same numbers twice has learned it is stuck, and `still running`
        // could not say that. It also lets an agent see spend BEFORE the budget is gone.
        Some("running") => format!(
            "Run {id} ({label}): still running — {} iterations, {} {} spent so far.\n",
            state["iterations"].as_u64().unwrap_or_default(),
            state["cost"].as_u64().unwrap_or_default(),
            state["unit"].as_str().unwrap_or("steps"),
        ),
        Some("done") => {
            let outcome = &state["outcome"];
            let reply = state["output"]
                .as_str()
                .map_or_else(String::new, |text| format!("  What it captured:\n{text}\n"));
            format!(
                "Run {id} ({label}): {}{} after {} iterations, {} {}.{}\n{reply}",
                outcome["state"].as_str().unwrap_or("?"),
                // ⚠ WHICH CEILING, because the three have three different remedies and an agent
                // told only `exhausted` has to guess which one to change. It is also the fact an
                // agent cannot derive: the ceilings it did not name came from the daemon's
                // defaults, so a run stopped by a default was stopped by a number it never saw.
                outcome[sprag_host::plugins::RUN_CEILING_KEY]
                    .as_str()
                    .map_or_else(String::new, |ceiling| {
                        format!(" — it ran out of {ceiling}")
                    }),
                outcome["iterations"].as_u64().unwrap_or_default(),
                outcome["cost"].as_u64().unwrap_or_default(),
                outcome["unit"].as_str().unwrap_or("steps"),
                outcome["failure"]
                    .as_str()
                    .map_or_else(String::new, |why| format!(" It failed: {why}.")),
            )
        }
        _ => format!(
            "Run {id} ({label}): {}.\n",
            state["status"].as_str().unwrap_or("in an unknown state"),
        ),
    }
}

/// `cancel_run`: stop one of this agent's own runs.
fn tool_cancel_run(args: &Value) -> Result<String, String> {
    let wanted = args
        .get("run")
        .and_then(Value::as_u64)
        .ok_or_else(|| "give the run id from orchestrate or list_runs as 'run'".to_owned())?;
    // OWNERSHIP BEFORE THE ACT, read from the daemon's own record rather than from the caller: a
    // run somebody else started is refused, and a run that never existed is told apart from one
    // that is not the caller's — two different things for the agent to do next.
    if !own_runs()?
        .iter()
        .any(|run| run["id"].as_u64() == Some(wanted))
    {
        return Err(format!(
            "Run {wanted} is not one of yours. list_runs shows the runs you started; a run started \
             by the person or by another agent is theirs to stop."
        ));
    }
    host_call(
        "scene/invoke",
        json!({
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::CANCEL_ACTION),
            "args": { "id": wanted },
        }),
    )?;
    Ok(format!(
        "Run {wanted} was asked to stop. It ends at its next step, so the pane it was driving is \
         left readable. list_runs says when it has.\n"
    ))
}

/// Dispatch a `tools/call`, wrapping the outcome into MCP `content`. A tool-level
/// failure is `isError: true` text (business error), NOT a JSON-RPC protocol error.
fn handle_tools_call(message: &Value) -> Value {
    let name = message
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome = match name {
        "list_panes" => tool_list_panes(),
        "list_windows" => tool_list_windows(),
        "open_window" => tool_open_window(&args),
        "select_window" => tool_select_window(&args),
        "close_window" => tool_close_window(&args),
        "display_message" => tool_display_message(&args),
        "rename_window" => tool_rename_window(&args),
        "list_sessions" => tool_list_sessions(),
        "pane_layout" => tool_pane_layout(&args),
        "pane_processes" => tool_pane_processes(&args),
        "pane_resources" => tool_pane_resources(&args),
        "grant_pane" => tool_grant_pane(&args),
        "machine_health" => tool_machine_health(),
        "read_pane" => tool_read_pane(&args),
        "read_last_command" => tool_read_last_command(&args),
        "read_pane_links" => tool_read_pane_links(&args),
        "read_pane_images" => tool_read_pane_images(&args),
        "find_in_pane" => tool_find_in_pane(&args),
        "wait_for_output" => tool_wait_for_output(&args),
        "regex_in_pane" => tool_regex_in_pane(&args),
        "agent_state" => tool_agent_state(&args),
        "agent_explain" => tool_agent_explain(&args),
        "wait_for_change" => tool_wait_for_change(&args),
        "write_pane" => tool_write_pane(&args),
        "send_keys" => tool_send_keys(&args),
        "open_pane" => tool_open_pane(&args),
        "close_pane" => tool_close_pane(&args),
        "rename_pane" => tool_rename_pane(&args),
        "select_pane" => tool_select_pane(&args),
        "swap_pane" => tool_swap_pane(&args),
        "resize_pane" => tool_resize_pane(&args),
        "zoom_pane" => tool_zoom_pane(&args),
        "break_pane" => tool_break_pane(&args),
        "join_pane" => tool_join_pane(&args),
        "move_pane" => tool_move_pane(&args),
        "resize_window" => tool_resize_window(&args),
        "orchestrate" => tool_orchestrate(&args),
        "list_runs" => tool_list_runs(),
        "cancel_run" => tool_cancel_run(&args),
        other => Err(no_such_tool(other)),
    };
    match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": format!("Error: {error}") }],
            "isError": true
        }),
    }
}

/// What an agent is told when it calls a tool this server does not serve.
///
/// # It used to be `unknown tool: X`, which is the sentence a TYPO gets
///
/// The exact defect R323 removed from the shell mouth, still standing on this one: a caller that
/// asked sprag to do something sprag DOES was told the name meant nothing. Three of the answers
/// below are about a real verb of this product, and only the fourth is a typo — and until the
/// vocabulary grew an agent axis there was no way to tell them apart, because there was nothing to
/// ask.
///
/// The name is normalised across the two spellings the product uses (`break_pane` here,
/// `break-pane` in a shell), because a caller that read `sprag --help` will type the other one and
/// that is a near miss rather than a mistake.
///
/// This is where [`sprag_host::vocabulary::NotAnAgents::why`] reaches a reader. A refusal rule that only a test can see is
/// a rule nobody was told, which is [`sprag_host::vocabulary`]'s own argument for the keyboard's
/// five reasons applied to its four.
fn no_such_tool(asked: &str) -> String {
    let spelled = asked.replace('_', "-");
    let Some(verb) = Verb::parse(&spelled) else {
        return format!(
            "unknown tool: {asked}. Call tools/list for the {} this server serves.",
            Verb::ALL.iter().flat_map(|verb| verb.tools()).count(),
        );
    };
    match verb.agent() {
        // A caller that typed the SHELL's spelling of a verb this surface does serve. Naming the
        // tool is the whole answer.
        Agent::Tools(tools) => format!(
            "there is no tool called {asked}. sprag calls that {} here — {} is the shell's \
             spelling of the same verb.",
            tools
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(" / "),
            verb.name(),
        ),
        Agent::NotBuilt => format!(
            "there is no tool called {asked}. sprag DOES have that verb — `sprag {}` runs it in a \
             shell — and no tool serves it to an agent yet. Nothing about it is refused; it is a \
             gap.",
            verb.name(),
        ),
        Agent::Cannot(why) => format!(
            "there is no tool called {asked}, and there will not be one: `{}` is a real verb of \
             sprag's and an agent cannot ask for it because {why}.",
            verb.name(),
            why = why.why(),
        ),
    }
}

// ---- Tools ---------------------------------------------------------------------

/// One pane as the host's pane-list reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneInfo {
    id: u64,
    /// The name a PERSON (or this pane's opener) gave it, `None` for a pane nobody named.
    ///
    /// The one STABLE handle on this surface. A pane's NUMBER is positional and moves when an
    /// earlier pane closes; the [`id`](Self::id) never moves but is an integer, so it cannot share
    /// the `pane` argument with a number. A name is a string, so it can — see [`pane_target_at`].
    ///
    /// # There is deliberately no `number` field
    ///
    /// A number is a property of a LISTING, not of a pane: it means "the Nth row of `list_panes`",
    /// and `list_panes` answers about ONE window. A row that carried its own number could be read
    /// out of a listing it does not belong to and would then name a different pane with a straight
    /// face — which is exactly what happened when R311 began reading OTHER windows' pane lists,
    /// numbering each from its own window's index. So a number is only ever formed by
    /// [`numbered`], from a position in a slice the caller is holding, and a pane one window over
    /// simply has none.
    name: Option<String>,
    title: String,
    command: String,
    cols: u64,
    rows: u64,
    /// The most recent attention notification the pane's child raised (`OSC 9` / `OSC
    /// 777;notify` / `OSC 99`), as a single display line, or `None` if it raised none — so an
    /// agent watching sibling panes learns which one wants attention.
    notification: Option<String>,
    /// The tmux monitor-bell count (`\a`) the pane's child has rung, `0` if none. Kept SEPARATE
    /// from the notification (a bell carries no text), so an agent sees a bell distinctly.
    bell: u64,
    /// The pane's shell-integration state (OSC 133): `Some("at_prompt")` / `Some("running")`, or
    /// `None` without integration — so an agent knows whether the shell is idle or running a
    /// command (it cannot tell from the pane text alone).
    shell: Option<String>,
    /// The last finished command's exit status (OSC 133 `D`), or `None` — lets an agent verify a
    /// command succeeded without parsing its output.
    exit_status: Option<i64>,
    /// The pane's live mouse-tracking level (DECSET 1000/1002/1003) as the host's `mouse` wire token
    /// (`"click"` / `"button"` / `"any"`), or `None` when the child is not tracking the mouse. An
    /// input-mode fact invisible in the pane's TEXT — an app that captures the pointer for itself —
    /// which tmux does not expose to an agent at all.
    mouse: Option<String>,
    /// Whether the pane's child has enabled focus reporting (DECSET 1004) — `true` when the app asked
    /// to be told it gained or lost focus (vim's external-edit check, a TUI that dims when inactive).
    /// Another invisible input-mode fact, orthogonal to [`PaneInfo::mouse`].
    focus_tracking: bool,
    /// The inline images (Kitty graphics / Sixel, R1404) the pane is displaying, each a summary
    /// {id, pixel size, anchor cell}. An agent cannot OCR an image, but CAN learn one is present and
    /// where — tmux shows no inline images at all.
    images: Vec<ImageInfo>,
    /// Whether this is the pane the session is ON — the daemon's active pane, which every attached
    /// client follows and which a pane verb given no target acts on. Exactly one pane of a window
    /// carries it. An agent reads it to know where a HUMAN is working before typing somewhere else.
    active: bool,
    /// What the AGENT running in the pane is doing (H3), or `None` for a pane no manifest claims —
    /// which is every ordinary shell. The one fact here that is about a SIBLING AI rather than about
    /// a program: it is how an agent learns that the pane next to it is waiting for a human.
    agent: Option<AgentInfo>,
    /// The HOST ID of the pane whose occupant asked for this one, `None` for a pane nobody claims —
    /// which is every pane a person made. Carried as the id rather than as a number because it is
    /// what the gate compares against ([`own_pane`] is an id too); it is rendered as a number.
    opened_by: Option<u64>,
}

/// One pane's agent verdict as an agent reads it — the wire's own `agent` object, field for field.
///
/// The state token is carried rather than glossed. A human surface renders `blocked` as a phrase
/// (`sprag_client::agent_phrase`, shared by both frontends so they cannot drift); this surface's reader
/// is a program that will branch on the value, so it gets the vocabulary and the tool description
/// teaches what each token means.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentInfo {
    /// `working` / `blocked` / `idle` — never absent, because the whole object is absent for a pane
    /// with no known state.
    state: String,
    /// Which manifest claims the pane (`claude`, `codex`), or `None` when a rule fired and a modal
    /// covered the fingerprint that would have named it (R251).
    name: Option<String>,
    /// Which RULE produced the state — what `agent_explain` exists to report (H3's D7: a gate that
    /// cannot say what it saw cannot be diagnosed).
    rule: Option<String>,
    /// WHO reported the state, for a verdict a process inside the pane asserted rather than one
    /// inferred from its screen. Mutually exclusive with [`rule`](Self::rule) in fact and additive
    /// on the wire: an agent reading this line has to be able to tell an authority from an
    /// inference, because only one of them is corrected by editing a manifest.
    source: Option<String>,
    /// Increments on a PUBLISHED change, so a poller tells "still blocked" from "blocked again"
    /// without diffing strings.
    seq: u64,
}

/// Parse the additive `agent` object (`{state, name?, rule?, source?, seq}`) from a panes-slot
/// entry.
///
/// The `state` token is what makes the value exist: a missing key, `null`, or an object without one
/// reads as `None` — "this host says nothing about an agent here". Never defaulted, because a
/// defaulted `idle` would tell an agent that a shell was a sibling waiting for input.
fn parse_agent_info(entry: &Value) -> Option<AgentInfo> {
    let agent = entry.get("agent")?;
    Some(AgentInfo {
        state: agent.get("state")?.as_str()?.to_owned(),
        name: agent.get("name").and_then(Value::as_str).map(str::to_owned),
        rule: agent.get("rule").and_then(Value::as_str).map(str::to_owned),
        source: agent
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        seq: agent.get("seq").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// One inline image a pane shows, as an agent reads it (R1404 Stage 5): its id, pixel size, and the
/// grid cell it is anchored at. The RGBA is not carried — a summary an agent uses to know an image
/// is present, not to reconstruct it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ImageInfo {
    id: u64,
    width: u64,
    height: u64,
    col: u64,
    row: u64,
}

/// Parse one panes-slot `images` summary entry (`{id,width,height,anchor:[col,row],seq}`).
fn parse_image_info(entry: &Value) -> Option<ImageInfo> {
    let anchor = entry.get("anchor")?.as_array()?;
    Some(ImageInfo {
        id: entry.get("id")?.as_u64()?,
        width: entry.get("width")?.as_u64()?,
        height: entry.get("height")?.as_u64()?,
        col: anchor.first()?.as_u64()?,
        row: anchor.get(1)?.as_u64()?,
    })
}

fn tool_list_panes() -> Result<String, String> {
    Ok(render_pane_list(&query_panes()?, own_pane()))
}

/// `list_windows` — the session's windows in the order the user arranged them, each with its panes.
///
/// **The reader R311 owed.** `wait_for_change` has always reported four window kinds (five since
/// R310's `windows_reordered`) and nothing on this surface could read a window, so an agent was
/// told about a subject it had no way to look at. It is also what makes a pane NAME usable across
/// windows: the resolver reaches any window of the session, and this is where an agent LEARNS the
/// names to reach for.
fn tool_list_windows() -> Result<String, String> {
    let names = query_window_names()?;
    if names.is_empty() {
        return Err("the host answered no windows for this session".to_owned());
    }
    let current = current_window_name();
    let mine = own_pane();
    let panes = query_session_panes()?;
    // Read from the SAME listing the names came from, so a window's provenance and its place in
    // the order describe one instant.
    let opened_by: std::collections::HashMap<String, Option<u64>> = query_windows()?
        .into_iter()
        .map(|window| (window.name, window.opened_by.map(|pane| pane.0)))
        .collect();
    let mut out = format!("{} window(s) in this sprag session:\n", names.len());
    for name in &names {
        let here: Vec<&PaneInfo> = panes
            .iter()
            .filter(|(window, _)| window == name)
            .map(|(_, pane)| pane)
            .collect();
        // YOURS is marked separately from CURRENT: they are different facts and an agent that
        // conflated them would think it was looking at what the user is looking at.
        let yours = mine.is_some_and(|id| here.iter().any(|pane| pane.id == id));
        let marks = [
            (current.as_deref() == Some(name.as_str())).then_some("current"),
            yours.then_some("you are here"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        out.push_str(&format!("  window {name}"));
        if !marks.is_empty() {
            out.push_str(&format!(" ({})", marks.join(", ")));
        }
        out.push_str(&format!(": {} pane(s)", here.len()));
        // WHOSE the window is, so an agent can see at a glance which ones it may close or rename —
        // `list_panes`' provenance line, one level up, and the answer to the question the two gated
        // window verbs raise. A window a PERSON made says nothing, because that is every window in
        // the ordinary case and a line on all of them would be noise.
        if let Some(opener) = opened_by.get(name.as_str()).copied().flatten() {
            let who = if Some(opener) == mine {
                " — opened by YOU (yours to close_window and rename_window)".to_owned()
            } else {
                format!(" — opened by {}", opener_subject(opener))
            };
            out.push_str(&who);
        }
        let named: Vec<String> = here
            .iter()
            .filter_map(|pane| pane.name.as_deref().map(|n| format!("{n:?}")))
            .collect();
        if !named.is_empty() {
            out.push_str(&format!(", named {}", named.join(", ")));
        }
        out.push('\n');
    }
    out.push_str(
        "A pane NAME reaches ANY window of this session, at every tool here that takes a `pane`. A \
         pane NUMBER means the Nth pane of YOUR window (list_panes) and never reaches further, so \
         a pane listed above is addressable by its name and not by a number. Ask pane_layout how \
         another window is arranged by naming a pane in it.\n",
    );
    Ok(out)
}

/// The name of the window this session is currently showing, or `None` if the host did not say.
fn current_window_name() -> Option<String> {
    let value = host_call(
        "scene/query",
        json!({ "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .ok()?;
    value.as_array()?.iter().find_map(|window| {
        window
            .get("current")?
            .as_bool()?
            .then(|| window.get("name")?.as_str().map(str::to_owned))?
    })
}

/// A window this surface has RESOLVED: its identity and name, whether the session is ON it, and WHO
/// ASKED for it — read in the ONE query that resolved it, for [`PaneRef`]'s reason.
///
/// Renamed from `WindowRef` at R330, when [`sprag_host::wire::WindowRef`] became the product-wide
/// name for a window ADDRESS. The two are different things and only one of them is a reference: this
/// is a reading, taken at an instant, that a tool then decides about.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedWindow {
    /// What a close is ADDRESSED to, or [`None`] from a daemon that publishes none.
    ///
    /// It is here because this surface RESOLVES a window, applies a policy to what it found, and
    /// only then acts — so the address has to survive that gap. See `close_window`.
    id: Option<sprag_terminal::WindowId>,
    name: String,
    current: bool,
    opened_by: Option<u64>,
    /// How many windows the session held at that same instant — what tells a close whether it
    /// would end the SESSION, from the same reading rather than a second one.
    siblings: usize,
}

/// Resolve a tool's `window` argument against one reading of the session's window list.
///
/// An AGENT addresses a window by NAME and only by name: a window has no number on this surface, so
/// there is one spelling and nothing to tell apart. That is why there is no `WindowAddress` beside
/// [`PaneAddress`](sprag_host::pane_address::PaneAddress) — the grammar that type exists for is the
/// two-spellings problem, and an agent's window argument does not have it.
///
/// ⚠ This doc said *"its id never leaves the daemon"* until R330, and that had been false since
/// R329 published `WindowInfo::id`. What stayed true is the half about the ARGUMENT; what changed is
/// that the resolution now CARRIES the identity, so a tool that acts after a policy check acts on
/// the window it checked.
fn resolve_window(args: &Value, key: &str) -> Result<ResolvedWindow, String> {
    let wanted = match args.get(key) {
        Some(Value::String(name)) => name.trim().to_owned(),
        Some(Value::Null) | None => {
            return Err(format!(
                "missing required argument '{key}': a window's NAME (call list_windows)"
            ));
        }
        Some(other) => return Err(format!("'{key}' must be a window's name, not {other}")),
    };
    let windows = query_windows()?;
    let siblings = windows.len();
    windows
        .into_iter()
        .find(|window| window.name == wanted)
        .map(|window| ResolvedWindow {
            id: window.id,
            name: window.name,
            current: window.current,
            opened_by: window.opened_by.map(|pane| pane.0),
            siblings,
        })
        .ok_or_else(|| {
            format!(
                "no window is called {wanted:?} in this session. Call list_windows to see the {} \
                 there {}.",
                siblings,
                if siblings == 1 { "is" } else { "are" },
            )
        })
}

/// Refuse a window this agent did not open — [`require_own_pane`]'s rule one level up, and the
/// reason [`sprag_terminal::Window::opened_by`] exists.
fn require_own_window(
    window: &ResolvedWindow,
    verb: &str,
    consequence: &str,
) -> Result<(), String> {
    match window.opened_by {
        Some(opener) if Some(opener) == own_pane() => Ok(()),
        Some(opener) => Err(format!(
            "window {} was opened by {}, not by you, so {verb} will not touch it. {consequence}",
            window.name,
            opener_subject(opener),
        )),
        None => Err(format!(
            "window {} was opened by a person, not by you, so {verb} will not touch it. \
             {consequence} Only a window you opened yourself with open_window is yours.",
            window.name,
        )),
    }
}

/// Where a newly-opened pane or window should start, resolved and CHECKED here so the caller gets a
/// sentence naming the path it asked for.
///
/// The action checks it too — it must, since this is not its only client — but from there the
/// refusal is a bare `Rejected` that cannot say which of its causes it was.
///
/// ONE function for both openers. `open_window` did not take a `cwd` at all until R313's audit,
/// which was an artifact of it being written after `open_pane` rather than a decision; copying the
/// parse would have made two answers to "is that a directory?" free to drift.
fn opt_cwd(args: &Value) -> Result<Option<PathBuf>, String> {
    let cwd = match args.get("cwd") {
        Some(Value::String(dir)) => Some(PathBuf::from(dir)),
        Some(other) => return Err(format!("'cwd' must be a string path, not {other}")),
        // This server is started by the agent's own client, so its working directory is the
        // agent's — derived rather than asked for, which is one fewer thing to get wrong.
        None => std::env::current_dir().ok(),
    };
    if let Some(dir) = &cwd
        && !dir.is_dir()
    {
        return Err(format!(
            "{} is not a directory this terminal can open a pane in",
            dir.display()
        ));
    }
    Ok(cwd)
}

/// `open_window` — a whole screenful of the agent's own, born DETACHED.
///
/// # Why detached, and why that is the round's decision rather than a flag
///
/// Because CREATING a place and SHOWING it are two acts and only the second is about the person.
/// The daemon's `new_window` always selected — measured at `37d3971`, `current` went `0` →
/// `agentwork` — so a tool that merely wrapped it would take the user's screen every single time an
/// agent decided to do some work. That is precisely the intrusion R294 gated the pane verbs
/// against, arriving one level up and with no gate at all. `select_window` is how an agent asks for
/// the screen, and asking is the point.
fn tool_open_window(args: &Value) -> Result<String, String> {
    let opener = own_pane().ok_or(
        "open_window needs to know which pane you are running in, and this server is not inside \
         one (no SPRAG_PANE published beside the socket it is talking to). Without it the daemon \
         cannot record the window as yours, and close_window would then refuse it.",
    )?;
    let name = match args.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(Value::Null) | None => None,
        Some(other) => return Err(format!("'name' must be a string, not {other}")),
    };
    // Through the grammar TYPE — see `WindowBirthAsk`. DETACHED is not a choice this tool offers:
    // an agent's window is always born quiet, and `select_window` is how it asks for the screen.
    let mut action_args = Value::Object(
        WindowBirthAsk(sprag_terminal::WindowBirth {
            detached: true,
            opened_by: Some(PaneId(opener)),
        })
        .to_args(),
    );
    if let Some(name) = &name {
        action_args["name"] = json!(name);
    }
    // `cwd` is `open_pane`'s argument, verbatim and through the SAME parser — its absence here was
    // an artifact of this tool being written after that one, not a decision: an agent that opens a
    // window to build in has exactly as much reason to say WHERE as one that opens a pane.
    if let Some(dir) = opt_cwd(args)? {
        let Some(dir) = dir.to_str() else {
            return Err(format!(
                "{} is not valid UTF-8, so it cannot be sent to the terminal",
                dir.display()
            ));
        };
        action_args["cwd"] = json!(dir);
    }
    let created = host_call_kinded(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_WINDOW_ACTION), "args": action_args }),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &match &name {
                Some(name) => format!(
                    "could not open a window called {name:?}: the name may already be taken by \
                     another window of this session, or be blank or malformed. Call list_windows \
                     to see which names are in use."
                ),
                None => "could not open a window".to_owned(),
            },
        )
    })?;
    let created = created
        .as_str()
        .ok_or("the host did not answer with the new window's name")?;
    // WHETHER THE PROVENANCE LANDED, read back off the window rather than assumed from the request
    // — `tool_open_pane`'s rule, for its reason: a daemon at the same wire protocol that predates
    // the key accepts the argument and records nothing, so promising the close would be a promise
    // this surface could not keep.
    let ours = query_windows()
        .ok()
        .and_then(|windows| {
            windows
                .into_iter()
                .find(|window| window.name == created)
                .map(|window| window.opened_by == Some(PaneId(opener)))
        })
        .unwrap_or(false);
    let mine = if ours {
        "It is yours to close_window and rename_window."
    } else {
        "WARNING: this terminal did not record it as opened by you, so close_window will refuse \
         it — the daemon predates the window provenance."
    };
    Ok(format!(
        "Opened window {created}, with a shell in it. The user did NOT move and cannot see it \
         yet — call select_window {{\"window\": {created:?}}} when you have something for them to \
         look at. {mine}\n\n{}",
        relisted_windows()
    ))
}

/// `select_window` — move the USER to another window. Ungated, like `select_pane`.
fn tool_select_window(args: &Value) -> Result<String, String> {
    let named = args.get(WindowRef::WINDOW_KEY);
    let stepped = args.get(SelectWindowAsk::RELATIVE_KEY);
    let ask = match (named, stepped) {
        (Some(Value::String(_)), None | Some(Value::Null)) => {
            // Resolved BEFORE the request so an unknown name is a sentence naming what exists,
            // where the daemon can only answer a payload-less `Rejected` (upstream PINION-PR82).
            // An agent TYPED a name, so a name is what this sends — the reading that argument has.
            SelectWindowAsk::At(WindowRef::Named(
                resolve_window(args, WindowRef::WINDOW_KEY)?.name,
            ))
        }
        (None | Some(Value::Null), Some(Value::String(word))) => {
            SelectWindowAsk::Step(OrderStep::from_wire(word).ok_or_else(|| {
                format!(
                    "'{}' must be one of {}, not {word:?}",
                    SelectWindowAsk::RELATIVE_KEY,
                    OrderStep::ALL.map(OrderStep::wire_str).join(", "),
                )
            })?)
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "'{}' and '{}' name the target two different ways; give one.",
                WindowRef::WINDOW_KEY,
                SelectWindowAsk::RELATIVE_KEY,
            ));
        }
        _ => {
            return Err(format!(
                "select_window needs '{}' (a window's NAME, from list_windows) or '{}' ({})",
                WindowRef::WINDOW_KEY,
                SelectWindowAsk::RELATIVE_KEY,
                OrderStep::ALL.map(OrderStep::wire_str).join(" / "),
            ));
        }
    };
    let landed = host_call(
        "scene/invoke",
        json!({ "path": mux_action_path(SELECT_WINDOW_ACTION), "args": ask.to_args() }),
    )?;
    let landed = landed
        .as_str()
        .ok_or("the host did not answer with the window it landed on")?;
    Ok(format!(
        "The user is now looking at window {landed}. Every attached client followed — this is \
         their whole screen, not a pane of it. Call list_panes to see what is in front of them \
         now; the numbers are that window's.",
    ))
}

/// `close_window` — end a window the agent opened, refusing a person's and refusing the last one.
fn tool_close_window(args: &Value) -> Result<String, String> {
    let window = resolve_window(args, WindowRef::WINDOW_KEY)?;
    require_own_window(
        &window,
        "close_window",
        "It may hold work nobody else can get back.",
    )?;
    // THE SESSION GUARD. R309 made a kill cascade: a session's last window ends the SESSION and the
    // last session ends the daemon. An agent tidying up its own workbench must not be able to end a
    // person's workspace, so the ordinary case is refused here — and the answer below reports the
    // daemon's own `Ended` word regardless, so the RACED case (a person closing the other window in
    // between) is told rather than hidden.
    if window.siblings <= 1 {
        return Err(format!(
            "window {} is this session's only window, so closing it would end the SESSION and \
             every pane in it. close_window will not do that. Leave it, or ask the person.",
            window.name,
        ));
    }
    // BY IDENTITY, and the key is `WindowRef`'s rather than `SelectWindowAsk`'s — borrowing another
    // action's constant is a bet that two grammars will never diverge, which is what this project's
    // one-spelling rule exists to stop.
    //
    // The identity is what makes the refusal above BINDING (R330): this tool looks the window up,
    // decides whether an agent may close it, and then acts. A name committed across that gap can
    // have moved to a window the guard never examined. A daemon that publishes no identity gets the
    // name, which is the reading it can honour and the only one it has.
    let mut args = serde_json::Map::new();
    match window.id {
        Some(id) => WindowRef::Picked(id).write(&mut args),
        None => WindowRef::Named(window.name.clone()).write(&mut args),
    }
    let answer = host_call(
        "scene/invoke",
        json!({
            "path": mux_action_path(KILL_WINDOW_ACTION),
            "args": Value::Object(args),
        }),
    )?;
    let beyond = Ended::from_wire(answer[ENDED_KEY].as_str().unwrap_or_default())
        .and_then(|ended| ended.beyond(Ended::Window))
        .map(|clause| {
            format!(
                " ⚠ It was the session's last window after all — somebody closed the other one \
                 between the check and the kill — so {clause}."
            )
        });
    Ok(format!(
        "Closed window {}, which you had opened, and every pane in it.{}\n\n{}",
        window.name,
        beyond.unwrap_or_default(),
        relisted_windows()
    ))
}

/// `rename_window` — rename a window the agent opened, refusing a person's.
/// `display_message` — say something to the PERSON at this terminal.
///
/// # Why an agent needs this at all, measured rather than assumed
///
/// Measured at `5acde43` by running the shipped binaries: an agent working in one pane had NO way to
/// put a sentence in front of the person in another. `send_keys` types into their program (their
/// command line, their editor) rather than telling them anything; `report_agent` carries a
/// three-word state to a window title; a pane's own OSC 9 reached the terminal front nowhere at all.
///
/// # It is not a substitute for working in your own pane
///
/// The description leans hard on WHEN, because the failure mode of a tool like this is an agent that
/// narrates. The daemon has no rate limit and deliberately none: a limit would be this process
/// deciding how often somebody else may be spoken to, and the honest place for that judgement is
/// here, in the words an agent reads before calling it.
fn tool_display_message(args: &Value) -> Result<String, String> {
    let text = match args.get("message") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => return Err(format!("'message' must be a string, not {other}")),
        None => return Err("display_message needs a 'message' to show".to_owned()),
    };
    // The same grammar the daemon enforces, read here so the agent is told WHICH rule it broke — a
    // wire refusal cannot carry a payload (PINION-PR82), and "invalid params" would send a caller
    // guessing at a newline it did not know it had.
    let text = sprag_host::report::MessageText::parse(&text)
        .map_err(|why| format!("that message cannot be shown: {why}"))?;
    let severity = match args.get("severity") {
        None | Some(Value::Null) => None,
        Some(Value::String(word)) => Some(
            sprag_host::report::Severity::parse(word)
                .ok_or_else(|| {
                    format!(
                        "'severity' is one of {}, not {word:?}",
                        sprag_host::report::Severity::words(),
                    )
                })?
                .word(),
        ),
        Some(other) => return Err(format!("'severity' must be a string, not {other}")),
    };
    let mut invoke_args = json!({ "text": text.as_str() });
    if let Some(severity) = severity {
        invoke_args["severity"] = Value::String(severity.to_owned());
    }
    let answer = host_call_kinded(
        "scene/invoke",
        json!({
            "path": mux_action_path(DISPLAY_MESSAGE_ACTION),
            "args": invoke_args,
        }),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            "could not show that message: this terminal may be older than the message surface, or \
             the message may be unacceptable (it must be one line, under 200 bytes, and free of \
             control characters).",
        )
    })?;
    let reached: Vec<&str> = answer["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // NOBODY IS A REAL ANSWER and it is said first, because it is the one an agent must act on: a
    // message shown to no one has not been delivered, and going on as though a person had read it is
    // exactly the mistake this tool exists to make impossible.
    if reached.is_empty() {
        return Ok(
            "NOBODY SAW IT: no window is attached to this terminal right now, so the message was \
             shown to no one. Do not treat it as delivered — if something needs a person, leave the \
             evidence where they will find it (in a pane, in a file, in your reply) as well."
                .to_owned(),
        );
    }
    Ok(format!(
        "Shown to {} attached window(s): {}. It is on their status line now and goes away on its \
         own unless you sent it as an `alert`, which stays until they press a key.",
        reached.len(),
        reached.join(", "),
    ))
}

fn tool_rename_window(args: &Value) -> Result<String, String> {
    let window = resolve_window(args, WindowRef::WINDOW_KEY)?;
    let new = match args.get("name") {
        Some(Value::String(name)) => name.clone(),
        Some(other) => return Err(format!("'name' must be a string, not {other}")),
        None => return Err("rename_window needs a 'name' to call the window".to_owned()),
    };
    require_own_window(
        &window,
        "rename_window",
        "Its name is what a person reads in the window list.",
    )?;
    host_call_kinded(
        "scene/invoke",
        json!({
            "path": mux_action_path(RENAME_WINDOW_ACTION),
            // `WindowRef`'s key, not another action's: `rename_window` addresses a window and this
            // is the one place this product spells that. Borrowing `SelectWindowAsk`'s was a bet
            // that two grammars never diverge (R330).
            "args": { WindowRef::WINDOW_KEY: window.name, "name": new },
        }),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "could not rename window {} to {new:?}: the name may already be taken by another \
                 window of this session, or be blank or malformed. Call list_windows to see which \
                 names are in use.",
                window.name,
            ),
        )
    })?;
    Ok(format!(
        "Window {} is now called {new:?}. That is its ADDRESS too — pass the new name to \
         select_window, close_window and pane_layout from here on.\n\n{}",
        window.name,
        relisted_windows()
    ))
}

/// `resize_window` — PIN a window's cell size, or hand it back (tmux `resize-window`).
///
/// # The two spellings an agent gets, and why not the other three
///
/// The verb has five: an exact rectangle, two client FOLDS (`-a`/`-A`), a relative adjustment and
/// the un-pin. An agent is not a client and reports no area, so the folds would ask it to name a
/// rectangle out of what the PEOPLE watching can see — a thing it cannot check and has no business
/// choosing. A relative adjustment needs a current size it would have to read back and race. What
/// is left is the pair a caller can be answerable for: say the rectangle, or say nothing and take
/// the forcing off.
///
/// # It reports the POLICY, and that is the whole point of the answer
///
/// A pin under a `window-size` that is not `manual` is stored and laid out over by nothing. A tool
/// that answered "resized" there would be telling an agent its columns had changed when
/// `read_pane` will show it the same width as before — the shape R331 measured and this is the
/// agent-facing half of. [`WindowPin::note`] is the same sentence a person gets, and the surface
/// that cannot see a screen is the one that most needs it.
fn tool_resize_window(args: &Value) -> Result<String, String> {
    let window = resolve_window(args, WindowRef::WINDOW_KEY)?;
    require_own_window(
        &window,
        "resize_window",
        "How big their window is belongs to them.",
    )?;
    let dimension = |key: &str| -> Result<Option<u16>, String> {
        match args.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
                .map(Some)
                .ok_or_else(|| {
                    format!("'{key}' must be a whole number of cells, 1 or more — {value} is not")
                }),
        }
    };
    // HALF a rectangle is refused whole rather than completed from whatever is pinned: a window
    // whose height came from a different decision than its width is a shape nobody chose, and the
    // wire refuses it anyway — said here so the agent is told which of its own arguments to fix.
    let size = match (dimension("cols")?, dimension("rows")?) {
        (Some(cols), Some(rows)) => SizeRequest::Exact(ClientSize { cols, rows }),
        (None, None) => SizeRequest::Clear,
        _ => {
            return Err(
                "resize_window needs 'cols' AND 'rows' together, or neither to un-pin: half a \
                 rectangle is a size nobody chose"
                    .to_owned(),
            );
        }
    };
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(
            json!({ "path": mux_action_path(RESIZE_WINDOW_ACTION) }),
            ResizeWindowAsk {
                window: Some(window.name.clone()),
                size,
            }
            .to_args(),
        ),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "could not resize window {}: no window of this session is called that. Call \
                 list_windows to see which names are in use.",
                window.name,
            ),
        )
    })?;
    let pinned = WindowPin::read(&answer);
    let did = match pinned.size {
        Some(size) => format!(
            "Window {} is pinned to {}x{} cells.",
            window.name, size.cols, size.rows
        ),
        None => format!(
            "Window {} is un-pinned and follows the clients watching it again.",
            window.name
        ),
    };
    // The NOTE is the half an agent cannot see for itself, so it leads the second paragraph rather
    // than being appended as a footnote: a caller that stopped reading after the first line would
    // otherwise act on a size that is not in force.
    Ok(match pinned.note() {
        Some(note) => format!("{did}\n\nNothing moved: {note}"),
        None => format!(
            "{did} Every pane in it is re-tiled to that rectangle, so read_pane now sees those \
             columns."
        ),
    })
}

/// The window listing, re-read after a verb that changed the set — [`relisted`]'s rule one level
/// up, and NOT `?`: the change already happened, and an error here is a broken connection rather
/// than a failed verb.
fn relisted_windows() -> String {
    tool_list_windows()
        .unwrap_or_else(|why| format!("(could not re-list the windows: {why} — call list_windows)"))
}

/// `list_sessions` — the daemon's sessions, so the three session events have a reader.
///
/// A READ and nothing else: these tools act on the agent's own session, and this answers what else
/// exists rather than offering a way in. That boundary is the user's, and a pane name's
/// registry-wide uniqueness is not a licence to cross it.
fn tool_list_sessions() -> Result<String, String> {
    let value = host_call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    let array = value
        .as_array()
        .ok_or("the host session list was not an array")?;
    if array.is_empty() {
        return Err("the host answered no sessions".to_owned());
    }
    let mut out = format!("{} session(s) on this sprag daemon:\n", array.len());
    for session in array {
        let name = session.get("name").and_then(Value::as_str).unwrap_or("?");
        let windows = session.get("windows").and_then(Value::as_u64).unwrap_or(0);
        let panes = session.get("panes").and_then(Value::as_u64).unwrap_or(0);
        let default = session
            .get("default")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        out.push_str(&format!(
            "  session {name:?}: {windows} window(s), {panes} pane(s)"
        ));
        if default {
            out.push_str(" (default)");
        }
        out.push('\n');
    }
    out.push_str("These tools act on YOUR session; this is what else the daemon holds.\n");
    Ok(out)
}

/// The whole numbered listing, as `list_panes` answers it.
///
/// Shared with the two tools that CHANGE the set ([`tool_open_pane`], [`tool_close_pane`]), because
/// their answer is this listing: a caller whose map of numbers has just been invalidated should not
/// have to make a second call to repair it, and it must be repaired with the same words it learned
/// them in. One rendering, so the two can never come to describe the same panes differently.
fn render_pane_list(panes: &[PaneInfo], here: Option<u64>) -> String {
    if panes.is_empty() {
        return "This sprag terminal has no panes.".to_owned();
    }
    // "in this window", not "in this terminal": a session holds several and this lists ONE of
    // them. The old wording described one window's worth as the whole thing, which is a wrong
    // answer that reads as complete — measured at `dac6ef7` on a 2-window, 3-pane session, where
    // it said "1 pane(s) in this sprag terminal".
    let mut out = format!(
        "{} pane(s) in this window (list_windows for the session's others):\n",
        panes.len()
    );
    for (index, pane) in panes.iter().enumerate() {
        out.push_str(&pane_summary(numbered(index), pane, panes, here));
    }
    out
}

/// Render ONE pane as its `list_panes` block — the header line plus an indented line per live
/// signal the pane raised. Each sub-line is emitted ONLY when its signal is present, so a resting
/// pane is just the header (mirrors the additive wire). Split out as a pure function so the
/// invisible-state lines (mouse / focus) are unit-testable without a live host.
fn pane_summary(number: usize, pane: &PaneInfo, panes: &[PaneInfo], here: Option<u64>) -> String {
    let title = if pane.title.is_empty() {
        "(none)".to_owned()
    } else {
        format!("{:?}", pane.title)
    };
    // The ACTIVE marker rides the header line rather than an indented one: it is a property OF the
    // pane's identity in the window, like its number, not a signal the pane raised.
    let active = if pane.active { " (active)" } else { "" };
    // The NAME rides the header too, and directly after the number it is the stable alternative to
    // — a reader meeting the two together is the point. Quoted like the title because a name may
    // hold a space, and so that `name="3rd"` cannot be misread as the number 3.
    let name = match &pane.name {
        Some(name) => format!(" name={name:?}"),
        None => String::new(),
    };
    let mut out = format!(
        "  pane {number}:{name} id={} {}x{} command={} title={}{active}\n",
        pane.id, pane.cols, pane.rows, pane.command, title
    );
    // Surface an attention notification on its own indented line, so an agent scanning the
    // list sees which sibling raised one (OSC 9 / 777;notify / 99).
    if let Some(note) = &pane.notification {
        out.push_str(&format!("      notification: {note}\n"));
    }
    // The tmux monitor-bell count, distinct from a notification (a bell carries no text).
    if pane.bell > 0 {
        out.push_str(&format!("      bell: rang {} time(s)\n", pane.bell));
    }
    // Shell-integration (OSC 133) summary: idle at a prompt vs running a command, and the last
    // command's exit status — what an agent needs to know a sibling's command finished, and
    // how, without parsing its output.
    if let Some(shell) = &pane.shell {
        let state = if shell == "running" {
            "running a command"
        } else {
            "at a prompt"
        };
        match pane.exit_status {
            Some(code) => {
                out.push_str(&format!(
                    "      shell: {state} (last command exit {code})\n"
                ));
            }
            None => out.push_str(&format!("      shell: {state}\n")),
        }
    }
    // Invisible INPUT-MODE state — what the app has asked the terminal to report, which is nowhere
    // in the pane's text and which tmux does not expose. An agent reads it to know the app is in a
    // pointer/focus-driven mode (so typing plain text may not behave as it would at a shell prompt).
    if let Some(mouse) = &pane.mouse {
        out.push_str(&format!("      mouse: tracking {}\n", mouse_phrase(mouse)));
    }
    if pane.focus_tracking {
        out.push_str("      focus: tracking focus in/out\n");
    }
    // WHO ASKED for the pane, absent for one nobody claims — which is every pane a person made.
    // The reader is an agent deciding what it may close, so the line answers that question first:
    // "you" is the only value `close_pane` accepts, and it is derived from the SAME comparison that
    // gate makes rather than from a second rule that could come to disagree with it.
    //
    // Named by NUMBER when this window holds the opener and by id when it does not, because a
    // number means nothing outside the listing it indexes — an opener in another window (or another
    // session; ids are registry-unique) is perfectly alive and simply not here.
    if let Some(opener) = pane.opened_by {
        let who = if Some(opener) == here {
            "you (yours to close)".to_owned()
        } else {
            match panes.iter().position(|p| p.id == opener) {
                Some(index) => format!("pane {}", numbered(index)),
                None => format!("pane id {opener}, not in this window"),
            }
        };
        out.push_str(&format!("      opened by: {who}\n"));
    }
    // The sibling AI, if the pane holds one (H3). Last because it is the only line here that is about
    // another agent rather than about a program: an agent scanning this list to find who needs a human
    // reads it, and every other line answers a question about the terminal.
    if let Some(agent) = &pane.agent {
        out.push_str(&format!("      agent: {}\n", agent_line(agent)));
    }
    out
}

/// One agent verdict as a `list_panes` / `agent_state` line: the state, whose it is, and which rule
/// said so.
///
/// Rendered from the wire's own fields with nothing invented, so a reader can act on the token and
/// `agent_explain` cannot disagree with `list_panes` about the same pane. `name` and `rule` are both
/// optional on the wire and both are simply omitted when absent — the additive discipline the pane
/// list itself follows, rather than a `none` a program would have to special-case.
fn agent_line(agent: &AgentInfo) -> String {
    let mut line = format!("state={}", agent.state);
    if let Some(name) = &agent.name {
        line.push_str(&format!(" name={name}"));
    }
    if let Some(rule) = &agent.rule {
        line.push_str(&format!(" rule={rule}"));
    }
    if let Some(source) = &agent.source {
        line.push_str(&format!(" source={source}"));
    }
    line.push_str(&format!(" seq={}", agent.seq));
    line
}

/// Render a pane's mouse-tracking wire token (`"click"` / `"button"` / `"any"`) into an agent-facing
/// phrase naming which pointer events the app captures. An unknown token passes through verbatim, so
/// a future tracking level still surfaces rather than vanishing.
fn mouse_phrase(token: &str) -> String {
    match token {
        "click" => "clicks (press/release)".to_owned(),
        "button" => "clicks + drag".to_owned(),
        "any" => "clicks + drag + motion".to_owned(),
        other => other.to_owned(),
    }
}

/// `pane_layout` — WHERE the panes sit, in the vocabulary this surface's own tools take.
///
/// The gap it closes: an agent could already make a pane active, read it and type into it, but had
/// no way to learn where any of them WERE — so it could not choose one by position, which is how a
/// human describes panes ("the one on the right"). The daemon has always known: the arrangement is a
/// published slot and adjacency is
/// [`LayoutWire::neighbor`](sprag_terminal::LayoutWire::neighbor), the same walk the user's
/// directional keybinding moves by. It was reachable over the wire only through the `select_pane`
/// ACTION, so the only way to ask "what is to my left" was to MOVE THE USER THERE.
///
/// # Two reads, and why the pane list comes first
///
/// The numbering every tool here takes is the pane pool's order ([`query_panes`]); the arrangement
/// is a different slot. A pane that exits between the two reads therefore holds a leaf this
/// numbering cannot name — and that is reported, in place, rather than dropped. Read the other way
/// round, the same pane would simply be missing from the drawing with nothing said, which is the
/// worse failure: a silent one.
///
/// This is NOT the join `sprag panes` refuses. That one would state a FACT assembled from two
/// instants (a zoom marked on a pane list), which can print a state that never existed. This one
/// TRANSLATES an identity — a pane's id and its number name the same pane — and its only failure
/// mode is a pane missing from one of the two sets, which is visible and said out loud. The moving
/// fact, which pane the user is typing into, is deliberately NOT here: it is `list_panes`'s answer.
fn tool_pane_layout(args: &Value) -> Result<String, String> {
    // `pane` names a WINDOW here, by naming something in it — the same address every other tool
    // takes rather than a second `window` grammar beside it. An agent that has just been told about
    // a pane one window over (`list_windows`, `wait_for_change`) can ask how that window sits
    // without first learning a different vocabulary, and there is one thing to get right.
    let elsewhere = resolve_optional_pane_ref(args)?.filter(|pane| pane.window.is_some());
    let (panes, window) = match &elsewhere {
        // The far window's panes carry NO numbers on this surface (they are not `list_panes`'s
        // rows), so the drawing names them by id and by name — see [`render_arrangement_answer`].
        Some(pane) => (
            query_window_panes(pane.window.as_deref().unwrap_or_default())?,
            pane.window.clone(),
        ),
        None => (query_panes()?, None),
    };
    let answer = host_call(
        "scene/query",
        windowed_params(mux_action_path(LAYOUT_SLOT), window.as_deref()),
    )?;
    // Through the SSOT type, never by walking the served arena by hand: it is a flat arena whose
    // nodes name their children by index, and a second reader of that encoding is a second thing
    // that can come to disagree with the daemon about what it means.
    let snapshot: LayoutSnapshot = serde_json::from_value(answer)
        .map_err(|error| format!("the host's arrangement did not parse: {error}"))?;
    Ok(render_arrangement_answer(
        &snapshot,
        &panes,
        own_pane(),
        window.as_deref(),
    ))
}

/// `grant_pane`: what ONE pane is ALLOWED of the machine — the setter beside `pane_resources`'
/// reading.
///
/// # Why an agent gets a SETTER here, re-derived rather than inherited
///
/// Most of this surface reads. This writes, and the argument that admits it is the one
/// `pane_resources` already made: an agent working in a pane is a participant in the contention,
/// not an observer of it. Told that a sibling pane is holding seven cores while its own work waits
/// a third of the time, an agent that can only report has to interrupt a person to change a number
/// — and the person's answer will be the number the agent already computed. The write is bounded
/// to a resource grant, it starves nothing (a weight is not a cap: a held-back pane still takes an
/// idle machine), and `memory.high` throttles rather than kills, so the worst outcome of a wrong
/// number is slow instead of lost.
///
/// # Why the answer is re-read and not echoed
///
/// [`crate::main`]'s host does that: the action re-reads the leaf. This function does not
/// re-implement it, and that is the point — an agent that was told its ceiling applied when the
/// host had no `memory` controller would report a fix it did not make.
fn tool_grant_pane(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    // The SAME authorship rule the other four writing tools keep, and it is not a formality here:
    // this changes what somebody's work is allowed to use. A person keeps `sprag grant`, which
    // reaches any pane, because the machine is theirs.
    //
    // ⚠ It inverts the obvious reading of the feature, and the inversion is the correct one. The
    // scenario the design document behind this describes is an AGENT running `make -j32` beside an
    // agent doing one thing at a time — so the greedy pane is an agent's OWN, and "hold yourself
    // back" is exactly the primitive that fixes it. "Hold your neighbour back" is a decision about
    // somebody's machine, and an agent that could make it would be taking cores from work it
    // cannot see.
    require_own_pane(
        &pane,
        "grant_pane",
        "Grant your OWN pane instead: holding back the pane running the heavy job is what frees \
         the machine, and if that job is not yours, tell the person what you measured.",
    )?;
    let mut action_args = serde_json::Map::new();
    action_args.insert("pane".to_owned(), json!(pane.id()));
    // Named one at a time rather than swept out of `args`, so an unknown key is never carried to
    // the daemon as if it meant something. The schema already refuses extras; this is the half that
    // does not depend on the caller having honoured it.
    for key in ["share", "memory", "processes"] {
        match args.get(key) {
            None | Some(Value::Null) => {}
            Some(value) => {
                let number = value
                    .as_u64()
                    .ok_or_else(|| format!("'{key}' must be a whole number, not {value}"))?;
                action_args.insert(key.to_owned(), json!(number));
            }
        }
    }
    if action_args.len() == 1 {
        return Err(
            "give at least one of 'share', 'memory' or 'processes' — a grant that sets nothing \
             would look like it worked"
                .to_owned(),
        );
    }
    let answer = host_call(
        "scene/invoke",
        json!({ "path": mux_action_path(GRANT_PANE_ACTION), "args": Value::Object(action_args) }),
    )?;
    let granted: sprag_terminal::Granted = serde_json::from_value(answer)
        .map_err(|error| format!("the daemon's answer was not a grant: {error}"))?;
    Ok(render_granted(&pane.subject(), granted))
}

/// What [`tool_grant_pane`] says, as a pure function of what the kernel answered.
///
/// Every row states what is IN FORCE and, where a control is missing, whose problem that is. An
/// agent reading `(this host's cgroup delegation has no memory controller)` knows not to try again
/// with a different number.
fn render_granted(subject: &str, granted: sprag_terminal::Granted) -> String {
    format!(
        "{subject} is now allowed:\n  {}\n  memory: {}\n  processes: {}\nThese are what the \
         kernel holds after the write, not what was asked for. Read pane_resources to see what \
         the pane actually does with them — a weight decides who waits when the machine is full, \
         and changes the TAIL of that waiting rather than the median.\n",
        agent_weight(granted.share),
        agent_ceiling(granted.memory, agent_bytes),
        agent_ceiling(granted.processes, agent_count_ceiling),
    )
}

/// One ceiling on its own, for the surface whose whole subject is the ceiling — the CLI's `ceiling`,
/// in the agent's words. [`agent_of`] is the version that hides behind a usage column; this one
/// cannot, because a blank here would not say whether the ceiling was removed or never took.
fn agent_ceiling(ceiling: Ceiling, spell: fn(u64) -> String) -> String {
    match ceiling {
        Ceiling::At(most) => spell(most),
        Ceiling::Uncapped => "no ceiling".to_owned(),
        Ceiling::NoController => {
            "no ceiling can be held here (this host's cgroup delegation is missing the \
             controller behind it)"
                .to_owned()
        }
    }
}

/// `pane_processes`: WHAT EACH PANE IS RUNNING — the job that owns its terminal, every process in
/// that job with its arguments, and the pane's terminal device. `pane` narrows to one pane.
///
/// # Why an agent gets this at all, re-derived rather than inherited
///
/// R285 declined an MCP tool for the zoom on the argument that *an agent reads and types; a zoom is
/// a thing you do FOR a human to look at*. R288 recorded that this argument INVERTS for a read, and
/// this is a read — but the inversion is not the reason, because "it is a read" would admit any
/// read at all. The reason is that an agent cannot get this fact another way. `list_panes` carries
/// the SPAWN label, frozen at the pane's birth. `read_pane` carries text, which a long build may
/// have scrolled away and which is identical between a silent build and an idle prompt. `agent_state`
/// answers about an AI agent, not about a compiler. The one thing that separates a busy pane from a
/// resting one is which process group owns its terminal, and until this tool existed nothing outside
/// the daemon could ask.
///
/// # Two reads, the pane list FIRST — [`tool_pane_layout`]'s discipline, for its reason
///
/// The numbering every tool here takes is the pane pool's order; the processes are a different slot.
/// A pane that exits between the two reads holds a row this numbering cannot name, and that is said
/// in place rather than dropped — numbering it anyway would hand back a number that now belongs to a
/// different pane. This translates an IDENTITY (a pane's id and its number name the same pane); it
/// does not assemble one fact from two instants.
fn tool_pane_processes(args: &Value) -> Result<String, String> {
    let wanted = resolve_optional_pane_ref(args)?;
    let here = query_panes()?;
    let session = query_session_panes()?;
    let registry = query_registry_tree()?;
    let answer = host_call(
        "scene/query",
        json!({ "path": mux_action_path(&pane_processes_at(0)) }),
    )?;
    // Through the SSOT type: a second reader of the served shape is a second thing that can come to
    // disagree with the daemon about what a field means.
    let wire: PaneProcessesWire = serde_json::from_value(answer)
        .map_err(|error| format!("the host's process reading did not parse: {error}"))?;
    Ok(render_processes_answer(
        &wire,
        &here,
        &session,
        &registry,
        wanted.as_ref(),
    ))
}

/// `pane_resources`: WHAT EACH PANE IS TAKING of the machine — cores held, waiting, memory,
/// processes. `pane` narrows to one pane.
///
/// # Why an agent gets this, re-derived
///
/// [`tool_pane_processes`]'s test, applied here: an agent cannot get this fact another way, and it
/// is a fact an agent acts on. An AI working in a pane that has become slow has exactly two
/// explanations available to it from every other tool — its own work is heavy, or the machine is
/// busy — and no way to tell them apart. The waiting figure separates them, and it is per PANE, so
/// it also names the neighbour responsible. That is the difference between an agent that retries
/// into a loaded machine and one that says *another pane is taking the CPU*.
///
/// The two reads and the pane-list-first ordering are [`tool_pane_processes`]'s, for its reasons.
/// The settle is [`settled`]'s — an agent asks once, so it must not be handed the empty first
/// reading a polling client would simply see replaced.
fn tool_pane_resources(args: &Value) -> Result<String, String> {
    let wanted = resolve_optional_pane_ref(args)?;
    let here = query_panes()?;
    let session = query_session_panes()?;
    let registry = query_registry_tree()?;
    let wire = settled(|| {
        let answer = host_call(
            "scene/query",
            json!({ "path": mux_action_path(&pane_resources_at(0)) }),
        )?;
        serde_json::from_value::<PaneResourcesWire>(answer)
            .map_err(|error| format!("the host's resource reading did not parse: {error}"))
    })?;
    Ok(render_resources_answer(
        &wire,
        &here,
        &session,
        &registry,
        wanted.as_ref(),
    ))
}

/// `machine_health`: WHAT IS WRONG with the machine the panes run on. Takes nothing.
///
/// # Why an agent gets this, re-derived
///
/// [`tool_pane_resources`]'s test, one level out. That tool lets an agent tell *my own work is
/// heavy* from *a neighbour is taking the machine*; it cannot tell either of those from *this
/// machine has less to give than it should*, and the investigation behind this feature found that
/// the third case was six sevenths of the real ones — a compiler cache the shells walked past,
/// kernel swap tuning, a delegation policy, a batch runner at equal weight. An agent that reads
/// every pane starved and no pane greedy has, from every other tool, no next question. This is the
/// next question.
///
/// It takes no argument at all, which is the point: a machine is not divided by session, and the
/// thing taking it may be in no session whatsoever.
fn tool_machine_health() -> Result<String, String> {
    let answer = host_call(
        "scene/query",
        json!({ "path": mux_action_path(&doctor_over(DOCTOR_WINDOW_MS)) }),
    )?;
    let report = serde_json::from_value::<Diagnosis>(answer)
        .map_err(|error| format!("the host's diagnosis did not parse: {error}"))?;
    Ok(render_health_answer(&report))
}

/// How long the agent's read asks the daemon to measure the competition over — the shared default,
/// so the CLI and an agent cannot come to disagree about what window a rate covers.
const DOCTOR_WINDOW_MS: u64 = DOCTOR_WINDOW.as_millis() as u64;

/// The text [`tool_machine_health`] returns, as a pure function of what was read
/// ([`render_processes_answer`]'s discipline).
///
/// The degraded rows FIRST, then everything else. An agent reads top-down and acts on the first
/// thing it understands, so a report that led with eight clean rows would bury the one fact it was
/// called for — and the clean rows still have to be there, because *checked and fine* is the answer
/// that stops an agent guessing at the same cause twice.
fn render_health_answer(report: &Diagnosis) -> String {
    let degraded: Vec<_> = report.degraded().collect();
    let mut out = String::new();
    if degraded.is_empty() {
        out.push_str(
            "Nothing is wrong with this machine that these checks can see. Every row below says \
             what it measured; a row marked `not measurable` was not checked, so do not read it as \
             healthy.\n\n",
        );
    } else {
        out.push_str(&format!(
            "{} of {} checks found something wrong with the MACHINE (not with sprag, and not with \
             any one pane):\n\n",
            degraded.len(),
            report.findings.len(),
        ));
        for finding in &degraded {
            let entry = finding.check.entry();
            out.push_str(&format!("{} — {}\n", entry.name, entry.asks));
            for row in finding.evidence.rows() {
                out.push_str(&format!("  {}: {}\n", row.of, row.is));
            }
            out.push_str(&format!("  read: {}\n", entry.source));
            out.push_str(&format!("  flagged when: {}\n", entry.criterion));
            // Named as the person's to run. An agent that read this as an instruction would be
            // applying a prescription this whole feature is bounded against applying.
            out.push_str(&format!(
                "  a person could: {} — tell them; do not do it yourself\n\n",
                entry.remedy,
            ));
        }
        out.push_str("The rest, with what each measured:\n\n");
    }
    for finding in &report.findings {
        if finding.verdict == Verdict::Degraded {
            continue;
        }
        let measured = finding
            .evidence
            .rows()
            .map(|row| format!("{}: {}", row.of, row.is))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "{} — {} ({measured})\n",
            finding.check.entry().name,
            match finding.verdict {
                Verdict::Healthy => "ok".to_owned(),
                // The reason, never a blank: an agent that reads "fine" and one that reads
                // "nobody could look" do different things next.
                Verdict::Blind(reason) => format!("not measurable: {reason}"),
                Verdict::Degraded => unreachable!("printed above"),
            },
        ));
    }
    out
}

/// The text [`tool_pane_resources`] returns, as a pure function of what was read — so every shape is
/// testable without a live host ([`render_processes_answer`]'s discipline).
fn render_resources_answer(
    wire: &PaneResourcesWire,
    here: &[PaneInfo],
    session: &[(String, PaneInfo)],
    registry: &[sprag_terminal::TreeSession],
    wanted: Option<&PaneRef>,
) -> String {
    let rows: Vec<_> = wire
        .panes
        .iter()
        // By ID and never by number, for [`render_processes_answer`]'s reason.
        .filter(|row| wanted.is_none_or(|pane| pane.id() == row.id))
        .collect();
    let mut out = format!(
        "What each pane is taking of the machine, sampled {} ms ago:\n\n",
        wire.sampled_ms_ago
    );
    if rows.is_empty() {
        out.push_str("No pane in this terminal has a row in the reading.\n");
        return out;
    }
    for row in rows {
        let name = process_row_subject(row.id, here, session, registry);
        match row.taken {
            Taken::Measured {
                cpu,
                waiting,
                memory,
                processes,
                granted,
            } => {
                out.push_str(&format!("{name}\n"));
                out.push_str(&format!("  holding {}\n", agent_cores(cpu)));
                out.push_str(&format!("  waiting {}\n", agent_waiting(waiting)));
                out.push_str(&format!(
                    "  {}, {}\n",
                    agent_of(agent_memory(memory), granted.memory, agent_bytes),
                    agent_of(
                        agent_processes(processes),
                        granted.processes,
                        agent_count_ceiling
                    )
                ));
                out.push_str(&format!("  allowed {}\n", agent_weight(granted.share)));
            }
            // The reason, never a blank: an agent that reads "unmeasured" and an agent that reads
            // "this whole daemon measures nothing" do different things next.
            Taken::Unmeasured { reason } => out.push_str(&format!("{name}\n  {reason}\n")),
        }
    }
    out.push_str(
        "\nHolding little CPU while waiting a lot means this pane is being starved by another \
         one; holding little while waiting for nothing means it has nothing to do. The window each \
         rate covers is stated, because a burst and a steady load look the same without it.\n",
    );
    out
}

/// The cores a pane holds, in the agent's words.
fn agent_cores(cpu: Cpu) -> String {
    match cpu {
        Cpu::Held {
            millicores,
            over_ms,
        } => format!(
            "{}.{:02} CPU cores, measured over the last {} ms",
            millicores / 1000,
            (millicores % 1000) / 10,
            over_ms
        ),
        Cpu::Settling => {
            "no rate yet — this pane has been sampled once; ask again in a moment".to_owned()
        }
    }
}

/// What a pane waited for, in the agent's words.
fn agent_waiting(waiting: Waiting) -> String {
    match waiting {
        Waiting::Measured {
            avg10,
            avg60,
            avg300,
        } => format!(
            "{avg10} of the last 10 seconds, {avg60} of the last minute, {avg300} of the last 5 \
             minutes"
        ),
        // Not "0%": this kernel keeps no pressure accounting at all, and reporting zero would tell
        // an agent that a pane which may have been starved of everything was never held up.
        Waiting::NotAccounted => {
            "unknown — this kernel keeps no pressure accounting, so starvation cannot be read"
                .to_owned()
        }
    }
}

/// A pane's memory, in the agent's words.
fn agent_memory(memory: Counted) -> String {
    match memory {
        Counted::Now(bytes) if bytes >= 1 << 20 => format!("{} MiB of memory", bytes >> 20),
        Counted::Now(bytes) => format!("{bytes} bytes of memory"),
        Counted::NoController => "memory unmeasured (no memory controller here)".to_owned(),
    }
}

/// A pane's process count, in the agent's words.
fn agent_processes(processes: Counted) -> String {
    match processes {
        Counted::Now(1) => "1 process".to_owned(),
        Counted::Now(many) => format!("{many} processes"),
        Counted::NoController => "process count unmeasured (no pids controller here)".to_owned(),
    }
}

/// A usage joined to the ceiling it is measured against, in the agent's words.
///
/// The agent needs this more sharply than a person does, because an agent DECIDES on it: told a
/// sibling pane holds 900 MiB, the useful next question is whether that is most of what it may have
/// or a rounding error, and those lead to opposite actions. An uncapped pane says so out loud rather
/// than going quiet, because "no ceiling" is itself the answer to *can this pane be told to use
/// less* — nothing is stopping it, so nothing will.
fn agent_of(usage: String, ceiling: Ceiling, spell: fn(u64) -> String) -> String {
    match ceiling {
        Ceiling::At(most) => format!("{usage}, of a ceiling of {}", spell(most)),
        Ceiling::Uncapped => format!("{usage}, with no ceiling set"),
        // Silent, because `usage` has already named the missing controller — see the CLI's `of`.
        Ceiling::NoController => usage,
    }
}

/// A process ceiling, bare — [`agent_of`] has already named the noun.
fn agent_count_ceiling(most: u64) -> String {
    most.to_string()
}

/// A ceiling in bytes, in the units [`agent_memory`] uses.
fn agent_bytes(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{} MiB", bytes >> 20)
    } else {
        format!("{bytes} bytes")
    }
}

/// The share of its level a pane is granted, in the agent's words.
///
/// **Stated as a weight and never as a predicted share of the machine.** The design behind this
/// feature measured both ways that would be wrong: a nominal 10:100 split came out at 18:82, and a
/// cgroup weighted 10 took every core it was offered once its sibling went idle. An agent told
/// "this pane may use 9% of the CPU" would act on a number that is false in both directions, so it
/// is told what the setting is and pointed at the cores actually held beside it.
fn agent_weight(share: Counted) -> String {
    match share {
        Counted::Now(weight) => format!(
            "a CPU weight of {weight} among its siblings — a weight is not a cap and not a ratio, \
             so read the cores held above for what it actually got"
        ),
        Counted::NoController => {
            "no CPU weight (this host's cgroup delegation has no cpu controller)".to_owned()
        }
    }
}

/// How a process row NAMES the pane it belongs to.
///
/// # The sentence this exists to stop saying
///
/// The reading is REGISTRY-WIDE and the numbering is one window's, so before R312 every row whose
/// pane lived in another window was rendered *"pane ? (id 1, gone since the pane list was read)"* —
/// **measured on a live two-window daemon, in the same line that went on to report that pane's tty
/// and its child's pid.** The residual sentence was written for a real race (a pane that exits
/// between the two reads) and had come to fire for the ordinary case instead, telling an agent that
/// a running pane was gone.
///
/// So a row is named against BOTH listings, and the three answers are three different facts: this
/// window numbers it, another window holds it, or nothing does — and only the last is the race.
fn process_row_subject(
    id: u64,
    here: &[PaneInfo],
    session: &[(String, PaneInfo)],
    registry: &[sprag_terminal::TreeSession],
) -> String {
    if let Some(index) = here.iter().position(|pane| pane.id == id) {
        return format!("pane {} (id {id})", numbered(index));
    }
    if let Some((window, _)) = session.iter().find(|(_, pane)| pane.id == id) {
        return format!("pane id {id} (window {window})");
    }
    // ANOTHER SESSION — the fourth answer, and R338 measured why it had to exist. R312 fixed this
    // sentence for a pane one WINDOW over and left it wrong one SESSION over, where it fired on a
    // live daemon: `pane_resources` reports on the whole machine, so a person running two sessions
    // was told the pane taking nineteen cores was "gone since the pane list was read". Through the
    // daemon's own reader, shared with the `sprag` CLI, so the two cannot disagree about which
    // session holds a pane.
    match sprag_host::wire::session_holding(registry, sprag_terminal::PaneId(id)) {
        Some(session) => format!("pane id {id} (session {session})"),
        // The residual of the reads, said rather than smoothed over — and now it means what it
        // says, because every window of every session has been looked in.
        None => format!("pane ? (id {id}, gone since the pane list was read)"),
    }
}

/// Every session this daemon holds, descending — the registry-wide listing the two machine-wide
/// tools name their rows against.
///
/// Read afresh rather than through [`our_session`]'s cache: that one answers a question whose answer
/// cannot change for this process (which pane am I in), and this one answers what the registry holds
/// right now.
fn query_registry_tree() -> Result<Vec<sprag_terminal::TreeSession>, String> {
    // UNSCOPED, because the subject is the set of sessions rather than one of them — the same
    // reason the slot itself is registry-wide. The fault's sentence is kept and its kind dropped:
    // every caller here answers a `String`.
    let answer = host_call_unscoped(
        "scene/query",
        json!({ "path": mux_action_path(sprag_host::wire::TREE_SLOT) }),
    )
    .map_err(|(said, _)| said)?;
    serde_json::from_value(answer)
        .map_err(|error| format!("the host's session tree did not parse: {error}"))
}

/// The text [`tool_pane_processes`] returns, as a pure function of what was read — so every shape is
/// testable without a live host, and the integration test can pin what an agent actually receives.
///
/// `wanted` is the pane the caller asked about, or `None` for all of them. `here` numbers the
/// caller's own window and `session` says which window holds every other row — see
/// [`process_row_subject`] for why both are needed.
fn render_processes_answer(
    wire: &PaneProcessesWire,
    here: &[PaneInfo],
    session: &[(String, PaneInfo)],
    registry: &[sprag_terminal::TreeSession],
    wanted: Option<&PaneRef>,
) -> String {
    let rows: Vec<_> = wire
        .panes
        .iter()
        // Narrowed by the pane's ID, never by its number. An id is registry-unique and never
        // reused, so the instant the caller's `pane` was resolved at and the instant this reading
        // was taken at cannot disagree about which pane is meant — which is what lets the resolver
        // make a read of its own without reintroducing a torn read.
        .filter(|row| wanted.is_none_or(|pane| pane.id() == row.id))
        .collect();
    let mut out = format!(
        "What each pane is running, sampled {} ms ago:\n\n",
        wire.sampled_ms_ago
    );
    if rows.is_empty() {
        out.push_str("No pane in this terminal has a row in the reading.\n");
        return out;
    }
    for row in rows {
        let name = process_row_subject(row.id, here, session, registry);
        let device = row
            .tty
            .as_deref()
            .map_or_else(String::new, |tty| format!(" on {tty}"));
        // A pane whose child has been reaped keeps its place and its final screen, so it is listed
        // rather than dropped, and it says it has no child instead of naming a pid it lost.
        let child = row.shell_pid.map_or_else(
            || " — no child process".to_owned(),
            |pid| format!(", child process {pid}"),
        );
        out.push_str(&format!("{name}{device}{child}\n"));
        let Some(job) = &row.foreground else {
            // Distinct from "no child": a live child whose terminal nobody owns is a real state,
            // and calling it the same thing would hide it.
            out.push_str("  nothing owns this pane's terminal\n");
            continue;
        };
        out.push_str(&format!("  running (job {}):\n", job.pgid));
        for process in &job.processes {
            // Quoted PER ARGUMENT: the wire carries the argument vector precisely because joining
            // it with bare spaces makes an argument containing a space indistinguishable from two,
            // so the one place that flattens it must not reintroduce that.
            let argv = process
                .argv
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            // Empty argv is a FACT the wire keeps (a zombie, a kernel thread), so it is said.
            let argv = if argv.is_empty() {
                "(no command line)".to_owned()
            } else {
                argv
            };
            out.push_str(&format!("    {} {}  {argv}\n", process.pid, process.name));
        }
    }
    out.push_str(
        "\nThe command in list_panes is what a pane was SPAWNED with and never changes; this is \
         what it is running now. A pane whose job is its own child process is sitting at a prompt.\n",
    );
    out
}

/// The text [`tool_pane_layout`] returns, as a pure function of what was read — so every shape is
/// testable without a live host, and the integration test can pin what an agent actually receives.
///
/// `here` is the pane this server runs in ([`own_pane`]), or `None` when it is not in one.
fn render_arrangement_answer(
    snapshot: &LayoutSnapshot,
    panes: &[PaneInfo],
    here: Option<u64>,
    window: Option<&str>,
) -> String {
    let entry_of = |pane: PaneId| panes.iter().find(|p| p.id == pane.0);
    // A NUMBER exists only for the caller's OWN window: it is `list_panes`'s row index, and
    // `list_panes` answers about that window. Drawing another window's arrangement therefore names
    // its panes by id and by name — the two handles that mean the same thing everywhere — rather
    // than by positions that would be read straight back as `pane: N` and land somewhere else.
    let number_of = |pane: PaneId| {
        window
            .is_none()
            .then(|| panes.iter().position(|p| p.id == pane.0).map(numbered))
            .flatten()
    };
    // The DRAWING's naming. Both integers, always: the number is what this surface's tools take, and
    // the id is what the same arrangement is called by `sprag layout`, the daemon's logs and the
    // user's own CLI — so an agent reporting to a human, and a human checking the agent, are not
    // holding two pictures that share no name.
    //
    // ...plus the pane's NAME when it has one, which costs no extra read (the pane list is already
    // in hand for the numbering) and is the whole reason a name exists: this drawing is where an
    // agent CHOOSES a pane, and a number chosen here goes stale the moment an earlier pane closes.
    // Handing back only numbers would answer "which pane" in the one vocabulary that moves.
    let label = |pane: PaneId| {
        let Some(entry) = entry_of(pane) else {
            // The residual of the two reads, said rather than smoothed over. Numbering it anyway
            // would hand the caller a number that now belongs to a DIFFERENT pane.
            return format!("pane ? (id {pane}, gone since the pane list was read)");
        };
        let mine = if here == Some(pane.0) {
            "  (you are here)"
        } else {
            ""
        };
        let named = match &entry.name {
            Some(name) => format!(" name={name:?}"),
            None => String::new(),
        };
        match number_of(pane) {
            Some(number) => format!("pane {number} (id {pane}){named}{mine}"),
            // No number, so the answer offers the handles that DO reach this window: the name if
            // the pane has one, and the id otherwise. It says so rather than leaving a reader to
            // wonder why the numbers stopped.
            None => format!("pane id {pane}{named}{mine}"),
        }
    };

    let mut out = match window {
        Some(window) => format!(
            "How WINDOW {window}'s panes are arranged (revision {}) — not your window, so its \
             panes carry no numbers here; address them by NAME:\n\n",
            snapshot.revision
        ),
        None => format!(
            "How YOUR WINDOW's panes are arranged (revision {}):\n\n",
            snapshot.revision
        ),
    };
    out.push_str(&arrangement::render(snapshot, &label));

    // Adjacency, from the arrangement just read — the SAME derivation `select-pane -L` moves by,
    // rather than one worked out from the drawing above. A caller that re-derived it would answer
    // differently on exactly the arrangements where the question is interesting (a column of panes
    // facing one divider, where the choice is by overlap rather than by shape).
    let tiled = snapshot.tree.panes();
    if tiled.len() > 1 {
        out.push_str(
            "\nWhich pane is next to which (a direction not listed has no pane that way — that \
             pane is at that edge of the window):\n",
        );
        for pane in &tiled {
            let sides: Vec<String> = PaneDir::ALL
                .iter()
                .filter_map(|dir| {
                    let found = snapshot.tree.neighbor(*pane, *dir)?;
                    Some(format!(
                        "{}={}",
                        dir.wire_str(),
                        short_name(found, &number_of)
                    ))
                })
                .collect();
            let sides = if sides.is_empty() {
                // Unreachable while more than one pane is tiled, and stated anyway rather than
                // printed as a bare colon: a pane with no neighbour at all is a real answer.
                "at every edge".to_owned()
            } else {
                sides.join(", ")
            };
            out.push_str(&format!("  {}: {sides}\n", short_name(*pane, &number_of)));
        }
    }
    out.push_str(
        "\nPass a pane NUMBER (not an id) to read_pane, write_pane, send_keys or select_pane. \
         Which pane the user is typing into right now is list_panes' answer, not this one. \
         To MOVE the user beside a pane, do not read a number from here and select it — that is \
         two moments; call select_pane with 'dir' plus 'from' or 'from_here: true' and the \
         terminal resolves it in one.\n",
    );
    out
}

/// A pane named for the neighbour table: its 1-based number, or its host id when the pane list this
/// answer was numbered from no longer holds it.
///
/// Shorter than the drawing's label on purpose — the table is read as a lookup, and the drawing
/// directly above it has already given every pane both of its names.
fn short_name(pane: PaneId, number_of: &impl Fn(PaneId) -> Option<usize>) -> String {
    number_of(pane).map_or_else(|| format!("pane id {pane}"), |n| format!("pane {n}"))
}

/// The pane this server is RUNNING IN, or `None` when it is not inside one.
///
/// The anchor the whole layout read needs: "the pane to my right" is unanswerable without it, and no
/// slot carries it — the fact is this PROCESS's, not the terminal's. That it is our own identity
/// rather than a slot read is why marking it is not the two-instant join this tool otherwise
/// refuses.
///
/// # It is the id published WITH the socket we are talking to, never merely the nearest one
///
/// The daemon publishes the pair together — `sprag_host::pane_env_source` writes a pane's id and its
/// daemon's address into one environment — and only the pair identifies a pane. Ids are per-daemon
/// and start at zero, so a box running two sprag terminals has two pane `1`s: a walk that took the
/// first `SPRAG_PANE` in reach could mark a pane of ANOTHER terminal, and it would mark a real,
/// plausible pane of this one rather than failing visibly. So a candidate environment is accepted
/// only when its address half is the socket this process actually asked. That also makes the
/// deliberate override honest — a client pointed at another daemon by `SPRAG_HOST_RPC_SOCK` gets no
/// mark instead of a wrong one.
///
/// `None` is a fine answer, and several ordinary situations produce it: an agent not inside a pane
/// at all, and a process that has OUTLIVED its pane (its id names a pane the pool no longer holds,
/// so the mark lands nowhere — see [`render_arrangement_answer`]).
/// The pane this server runs in, RESOLVED — so an origin that means "here" is the same kind of
/// thing as an origin the caller named, and the two arms of [`select_origin`] answer one type.
fn own_pane_ref() -> Option<PaneRef> {
    let id = own_pane()?;
    let panes = query_panes().ok()?;
    let index = panes.iter().position(|pane| pane.id == id)?;
    Some(PaneRef {
        number: Some(numbered(index)),
        window: None,
        info: panes[index].clone(),
    })
}

fn own_pane() -> Option<u64> {
    let sock = host_sock()?;
    // The address half, compared as a PATH rather than as text, so the two spellings of one socket
    // an environment can carry do not read as two daemons.
    let published_with_sock = |pane: Option<String>, address: Option<String>| -> Option<u64> {
        let (pane, address) = (pane?, address?);
        (std::path::Path::new(&address) == sock).then_some(())?;
        pane.parse().ok()
    };
    let own = |key: &str| std::env::var(key).ok();
    if let Some(id) = published_with_sock(own(PANE_ENV_VAR), own(SOCK_ENV)) {
        return Some(id);
    }
    ancestor_pids().into_iter().find_map(|pid| {
        published_with_sock(
            read_proc_env(pid, PANE_ENV_VAR),
            read_proc_env(pid, SOCK_ENV),
        )
    })
}

fn tool_read_pane(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let value = host_call(
        "scene/query",
        pane_params(&pane, pane_input_path(pane.id(), FULL_TEXT_SLOT)),
    )?;
    let text = value
        .as_str()
        .ok_or("the host did not return pane text")?
        .to_owned();
    match args.get("tail_lines").and_then(Value::as_u64) {
        Some(n) => Ok(last_n_lines(&text, n as usize)),
        None => Ok(text),
    }
}

/// Read the pane's LAST command sliced at its OSC 133 marks — the command line, its output,
/// and its exit status — rendered as a readable block. A `null` slot means the pane's shell
/// has no OSC 133 integration; the agent is told to fall back to `read_pane`.
fn tool_read_last_command(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let value = host_call(
        "scene/query",
        pane_params(&pane, pane_input_path(pane.id(), LAST_COMMAND_SLOT)),
    )?;
    if value.is_null() {
        return Ok(
            "No shell-integration command boundaries in this pane (its shell may not \
             emit OSC 133 marks); use read_pane for the raw screen instead."
                .to_owned(),
        );
    }
    let command = value.get("command").and_then(Value::as_str).unwrap_or("");
    let output = value.get("output").and_then(Value::as_str).unwrap_or("");
    let status = if value.get("running").and_then(Value::as_bool) == Some(true) {
        "still running".to_owned()
    } else {
        match value.get("exit_status").and_then(Value::as_i64) {
            Some(code) => format!("exit {code}"),
            None => "finished (exit status not reported)".to_owned(),
        }
    };
    Ok(format!("{command}\n[{status}]\n--- output ---\n{output}"))
}

/// List a pane's visible OSC-8 hyperlinks — each link's displayed text and its URI — so an agent
/// reads a link's destination as data. tmux's `capture-pane` cannot: it flattens OSC 8 to plain
/// text, dropping the URI entirely.
fn tool_read_pane_links(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let value = host_call(
        "scene/query",
        pane_params(&pane, pane_input_path(pane.id(), LINKS_SLOT)),
    )?;
    let runs = value
        .as_array()
        .ok_or("the host did not return a links array")?;
    if runs.is_empty() {
        return Ok("This pane shows no OSC-8 hyperlinks.".to_owned());
    }
    let mut out = format!("{} link(s) in this pane:\n", runs.len());
    for run in runs {
        let text = run.get("text").and_then(Value::as_str).unwrap_or("");
        let uri = run.get("uri").and_then(Value::as_str).unwrap_or("");
        match run.get("id").and_then(Value::as_str) {
            Some(id) => out.push_str(&format!("  {text:?} -> {uri} (id={id})\n")),
            None => out.push_str(&format!("  {text:?} -> {uri}\n")),
        }
    }
    Ok(out)
}

/// List the inline images (Kitty graphics / Sixel) a pane is displaying — each image's id, pixel
/// size, and anchor cell. An agent can't OCR an image, but CAN learn one is present and where; tmux
/// shows no inline images at all, let alone as data.
fn tool_read_pane_images(args: &Value) -> Result<String, String> {
    // The target is parsed, then resolved against the listing THIS call reads — never through a
    // helper that queries on its own. Two queries here would resolve the caller's `pane` against
    // one instant and read the pane at another, which is the torn read a NAME exists to prevent.
    let pane = resolve_pane_ref(args)?;
    // The images ride on the resolved pane, from the reading that resolved it. Reading the listing
    // a second time here is what made this tool window-local: it queried the CALLER's window and
    // asked it about a pane one window over.
    if pane.info.images.is_empty() {
        return Ok(format!("{} shows no inline images.", pane.subject()));
    }
    let mut out = format!(
        "{} image(s) in {}:\n",
        pane.info.images.len(),
        pane.subject()
    );
    for img in &pane.info.images {
        out.push_str(&format!(
            "  image #{}: {}x{} px at cell ({}, {})\n",
            img.id, img.width, img.height, img.col, img.row
        ));
    }
    Ok(out)
}

/// Search a pane through the host's `find.<needle>` query family and render the matching LINES for
/// an agent — `LINE: text`, one per matching line.
///
/// Reads the answer's `lines` (deduped) rather than its `matches` (coordinates): an agent quotes
/// text and line numbers, and cell columns would be noise it cannot act on. No second search lives
/// here — the host owns what matches, so this tool, the `sprag find` CLI and the GUI's highlight
/// cannot disagree. A capped answer says so in the rendered text, since an agent that believed a
/// truncated list was complete would conclude something false about the pane.
fn tool_find_in_pane(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let needle = args
        .get("needle")
        .and_then(Value::as_str)
        .filter(|needle| !needle.is_empty())
        .ok_or("find_in_pane needs a non-empty `needle`")?;
    search_pane(&pane, &find_slot_for(needle), needle)
}

/// `regex_in_pane` — the same search read as a REGULAR EXPRESSION.
///
/// A separate tool rather than a flag on `find_in_pane`, all the way up from the wire: a needle and
/// a pattern are separate languages in which the same string means different things, so which one
/// an agent means is expressed by WHICH TOOL it calls, not by an argument that could be defaulted,
/// forgotten, or carried over from a previous call.
fn tool_regex_in_pane(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|pattern| !pattern.is_empty())
        .ok_or("regex_in_pane needs a non-empty `pattern`")?;
    search_pane(&pane, &regex_slot_for(pattern), pattern)
}

/// Query pane `id` at `slot` and render the matching lines as `LINE: text`.
///
/// The ONE renderer both search tools use, so a literal hit and a regex hit read identically to an
/// agent — only the language of `wanted` (echoed in the no-match message) differs. Neither tool
/// implements a search: both read a host query, so they agree with the CLI and the GUI highlight.
fn search_pane(pane: &PaneRef, slot: &str, wanted: &str) -> Result<String, String> {
    let value = host_call(
        "scene/query",
        pane_params(pane, pane_input_path(pane.id(), slot)),
    )?;
    let found: PaneFind =
        serde_json::from_value(value).map_err(|error| format!("malformed find answer: {error}"))?;
    // A refused pattern is an ERROR, not an empty result: "your pattern is wrong" and "nothing
    // matched" are different answers, and an agent that cannot tell them apart will retry forever.
    if let Some(error) = found.error {
        return Err(format!("invalid pattern {wanted:?}: {error}"));
    }
    if found.lines.is_empty() {
        return Ok(match &pane.window {
            // The WINDOW is named only when the pane is not the caller's own: an agent that
            // searched a sibling window and found nothing is owed which screen was searched.
            Some(window) => format!(
                "no matches for {wanted:?} in pane {} (window {window})",
                pane.id()
            ),
            None => format!("no matches for {wanted:?} in pane {}", pane.id()),
        });
    }
    let mut out = String::new();
    for line in &found.lines {
        out.push_str(&format!("{}: {}\n", line.line, line.text));
    }
    if found.truncated {
        out.push_str("(the search hit its cap; later matches were not scanned)\n");
    }
    Ok(out)
}

/// `wait_for_output` — BLOCK until a pane's retained output matches, then answer with the matching
/// lines.
///
/// ## The tool `find_in_pane` could not be, and the loop it removes
///
/// `find_in_pane` answers "does it say this NOW". An agent that wants "tell me WHEN it says this"
/// has had to call it in a loop — a poll, against a terminal, from the surface whose other wait
/// tool's description opens by telling an agent not to poll. `wait_for_change` does not cover it
/// either, and deliberately: output is not a change there (a record per PTY batch would evict the
/// journal's ring at output rate), so it will never return for a line appearing on a screen.
///
/// So this is the third wait, and it is the one that completes the workflow the others were built
/// for: `open_pane` to get a workbench, `write_pane` to start something, and this to be told the
/// moment it says what you were waiting for.
///
/// ## It searches what the pane KEPT, not what it is showing
///
/// The daemon's search reads scrollback plus visible, so a line printed and scrolled away while the
/// agent was not looking still matches. That is the property a re-reading poll cannot have — it can
/// only ever see the screen as it is when it looks — and it is why "wait for the build to print
/// DONE" is answerable at all on a pane that goes on producing afterwards.
///
/// ## One reading of the listing, and one search language per call
///
/// The pane comes from [`resolve_pane_ref`], the one resolver — so what crosses from the resolving
/// read to the wait is an ID, which never moves. `needle` and `pattern` are separate arguments
/// because they are separate languages, exactly as `find_in_pane` and `regex_in_pane` are separate
/// tools.
fn tool_wait_for_output(args: &Value) -> Result<String, String> {
    let timeout = match args.get("timeout_seconds") {
        None => Duration::from_secs(60),
        Some(value) => {
            let seconds = value
                .as_u64()
                .ok_or("timeout_seconds must be a whole number of seconds")?;
            if !(1..=600).contains(&seconds) {
                return Err("timeout_seconds must be between 1 and 600".to_owned());
            }
            Duration::from_secs(seconds)
        }
    };
    let wanted = args.get("needle").and_then(Value::as_str);
    let pattern = args.get("pattern").and_then(Value::as_str);
    let (key, wanted) = match (wanted, pattern) {
        (Some(needle), None) if !needle.is_empty() => (NEEDLE_PARAM, needle),
        (None, Some(pattern)) if !pattern.is_empty() => (PATTERN_PARAM, pattern),
        (Some(_), Some(_)) => {
            return Err(
                "give `needle` (literal text) or `pattern` (a regular expression), never \
                        both — they are different search languages"
                    .to_owned(),
            );
        }
        _ => {
            return Err(
                "wait_for_output needs a non-empty `needle` (literal text) or `pattern` \
                        (a regular expression)"
                    .to_owned(),
            );
        }
    };
    // Through the one resolver, so this wait reaches as far as the daemon's does. Measured:
    // `handle_output_wait` checks the pane against `scope.session()`, not against a window, so the
    // park was ALWAYS session-wide and only the client's name lookup was narrow.
    let pane = resolve_pane_ref(args)?;
    let (subject, id) = (pane.subject(), pane.id());

    let sock = host_sock().ok_or_else(|| {
        "not inside a sprag terminal (no SPRAG_HOST_RPC_SOCK in this process or any \
         ancestor); these pane tools do not apply to this session"
            .to_owned()
    })?;
    let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)
        .map_err(|e| format!("cannot reach the sprag host at {}: {e}", sock.display()))?;
    // The caller's timeout IS the read deadline, and it is the ONLY deadline: the daemon carries
    // none, so closing this connection is what releases the park (`sprag_host::notify`).
    conn.set_read_deadline(Some(timeout))
        .map_err(|e| format!("cannot set the wait timeout: {e}"))?;

    let params = json!({ PANE_PARAM: id, key: wanted });
    let answer = match conn.try_call(PANE_WAIT_OUTPUT_METHOD, params) {
        Ok(answer) => answer,
        // The caller's own mistake, in the daemon's own words — reaching the agent as that sentence
        // rather than behind `host rpc error:`. Matched on the fault's CODE, never its rendering:
        // a substring test against a rendering is a test against a presentation decision, which is
        // the rule R292 wrote down and R295 broke.
        Err(CallError::Fault(fault)) if fault.code == INVALID_PARAMS => {
            return Err(fault
                .data
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(&fault.message)
                .to_owned());
        }
        // A connection that trips its deadline is finished, and nothing having happened is not an
        // error: it is the answer "not within the time you gave me", which is what the agent asked.
        //
        // ⚠ THE DEADLINE IS TOLD APART FROM A REAL FAILURE, and reading the rendered answer is what
        // put this here. Reporting every transport error as "not yet" would tell an agent whose
        // daemon had died that its build is still running — the most expensive lie this surface can
        // tell, because the agent's correct response is to wait longer. A read deadline surfaces as
        // `WouldBlock` or `TimedOut` (both spellings the platforms use); anything else is a
        // failure and says so.
        //
        // The error itself is NOT rendered into the timeout sentence. It is a Rust-shaped
        // `Transport(Custom { .. })` line that says nothing an agent can act on, beside a sentence
        // that already says nothing failed — debt item 20's class, on this surface, avoided at
        // birth rather than registered.
        Err(CallError::Transport(error))
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(format!(
                "{subject} has not printed {wanted:?} yet (waited {}s; nothing failed). Call \
                 again to keep waiting, or read the pane to see what it IS doing.",
                timeout.as_secs()
            ));
        }
        Err(CallError::Transport(error)) => {
            return Err(format!(
                "the wait on {subject} did not complete: {error}. This is NOT 'it has not \
                 happened yet' — the terminal could not be reached."
            ));
        }
        Err(CallError::Fault(fault)) => return Err(fault.to_string()),
    };
    let found: PaneFind = serde_json::from_value(answer["find"].clone())
        .map_err(|error| format!("malformed find answer: {error}"))?;
    // A refused pattern is an ERROR, not an empty result — `search_pane`'s own rule: an agent that
    // cannot tell "your pattern is wrong" from "nothing matched yet" will wait forever on a typo.
    if let Some(error) = found.error {
        return Err(format!("invalid pattern {wanted:?}: {error}"));
    }
    let mut out = format!("{subject} printed {wanted:?}:\n");
    for line in &found.lines {
        out.push_str(&format!("{}: {}\n", line.line, line.text));
    }
    if found.truncated {
        out.push_str("(the search hit its cap; later matches were not scanned)\n");
    }
    Ok(out)
}

fn tool_write_pane(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let id = pane.id();
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or("missing required string argument 'text'")?;
    host_call(
        "scene/invoke",
        with_args(
            pane_params(&pane, pane_input_path(id, TEXT_ACTION)),
            json!({ "text": text }),
        ),
    )?;
    let enter = args.get("enter").and_then(Value::as_bool).unwrap_or(true);
    if enter {
        host_call(
            "scene/invoke",
            with_args(
                pane_params(&pane, pane_input_path(id, KEY_ACTION)),
                json!({ "key": "Enter" }),
            ),
        )?;
    }
    Ok(format!(
        "Wrote {} byte(s) to {}{}.",
        text.len(),
        pane.subject(),
        if enter { " and pressed Enter" } else { "" }
    ))
}

/// `open_pane` — a pane of this agent's own, recorded as opened BY the agent's pane.
///
/// # Why an agent gets a create verb when it does not get `move` / `swap` / `zoom`
///
/// Those three decide what a PERSON looks at, and an agent has no basis for the decision. This one
/// is not about the person's arrangement at all: it is the agent's workbench. Everything this
/// surface gained across the four rounds before it — read WHERE the panes are, read WHAT one is
/// running, be told when a job changes, wait for exactly the change named — presupposes a pane a
/// human happened to open, so "run the build over there and wait for it" had no first step.
///
/// # Why it APPENDS, and takes no placement
///
/// [`SPAWN_ACTION`] is the birth that states no opinion about the
/// arrangement; a directional split would have the agent choosing how to divide somebody's screen,
/// which is the decision declined above. The person can move it afterwards with the verbs written
/// for them.
///
/// # Why the answer re-lists every pane
///
/// This surface addresses panes by their 1-based POSITION, so the agent's map of "which number is
/// which pane" is only as good as its last `list_panes`. An open appends, so the existing numbers
/// do NOT move — but the new pane's number is the one fact the caller needs and cannot derive, and
/// re-listing is how [`tool_close_pane`] (where the numbers really do shift) answers too. One shape
/// for both writes, so a caller never has to remember which of them invalidated what.
fn tool_open_pane(args: &Value) -> Result<String, String> {
    let opener = own_pane().ok_or(
        "open_pane needs to know which pane to record as the opener, and this server is not \
         running inside a sprag pane (no SPRAG_PANE published beside the socket it is talking \
         to). A pane opened with nobody answerable for it could never be closed by this tool, so \
         it is refused rather than left behind. The user can open one with `sprag split-window`.",
    )?;
    let cwd = opt_cwd(args)?;
    // The name is passed THROUGH rather than validated here: the daemon owns the rules, and a
    // second copy of them in this crate would be a second answer that can drift. What this does own
    // is the sentence — the daemon's refusal is a bare `Rejected` that cannot say which rule it was.
    let name = match args.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(other) => return Err(format!("'name' must be a string, not {other}")),
        None => None,
    };
    let mut spawn_args = json!({ "opened_by": opener });
    if let Some(name) = &name {
        spawn_args["name"] = json!(name);
    }
    if let Some(dir) = &cwd {
        let Some(dir) = dir.to_str() else {
            return Err(format!(
                "{} is not valid UTF-8, so it cannot be sent to the terminal",
                dir.display()
            ));
        };
        spawn_args["cwd"] = json!(dir);
    }
    let id = host_call_kinded(
        "scene/invoke",
        json!({ "path": mux_action_path(SPAWN_ACTION), "args": spawn_args }),
    )
    // A birth that carries a name has a second way to be refused, and the daemon cannot say which
    // (`InvokeError::Rejected` has no payload — upstream PINION-PR82). So the sentence names the
    // causes the caller can act on, and REPLACES the daemon's rather than appending to it: what it
    // would otherwise append to is `host rpc error: InvokeRejected`, a Rust variant name — the
    // exact leak R283 measured and fixed on the CLI, reaching an AGENT here.
    .map_err(|why| match &name {
        Some(name) => refusal_sentence(
            &why,
            &format!(
                "could not open a pane called {name:?}: the name may already be taken by another \
                 pane, or be blank, over 80 bytes, all digits, or contain a control character. \
                 Call list_panes to see which names are in use."
            ),
        ),
        None => why.0,
    })?
    .as_u64()
    .ok_or("the host did not answer with a new pane id")?;

    // NOT `?`, for [`relisted`]'s reason: the pane EXISTS from here on, so a failed re-read must
    // still tell the caller that — a call that answered "error" would leave a pane running that its
    // opener does not know it has, which is the litter this whole design exists to prevent.
    let panes = query_panes().unwrap_or_default();
    let born = panes.iter().position(|p| p.id == id);
    let row = born.map(|index| &panes[index]);
    let number = born
        .map(|index| numbered(index).to_string())
        // The pane was born — the host answered with its id — so a listing that no longer holds it
        // means it has ALREADY gone (a shell that exec'd and exited), or could not be re-read.
        // Reported with the id that always addresses it, never guessed at.
        .unwrap_or_else(|| format!("? (id {id}, not in the pane list just read)"));
    let where_it_is = cwd.map_or_else(String::new, |dir| format!(" in {}", dir.display()));
    // WHETHER THE PROVENANCE ACTUALLY LANDED, read back off the pane rather than assumed from the
    // request that was sent. Not defensive noise — the skew run that proved this key additive is
    // what produced the case: a daemon at the SAME wire protocol that predates `opened_by` accepts
    // the argument and records nothing, so it neither refuses the client nor honours it. Saying
    // "close_pane will let you close it" from the request alone would be a promise this tool would
    // then break, and the caller would find out only when its cleanup was refused.
    let ours = row.is_some_and(|p| p.opened_by == Some(opener));
    // And whether the NAME landed, read back off the pane for the same reason and with the same
    // hazard: an additive ARGUMENT an old daemon drops is a silent no-op, where an additive FIELD an
    // old client ignores is harmless. The two are not symmetric, which is the general shape R294's
    // skew run produced — so every argument this tool sends is checked in the answer, not assumed.
    // Compared against the TRIMMED name the daemon would have stored, since that is what it records.
    let named = name.as_ref().map(|asked| {
        let landed = row.and_then(|p| p.name.as_deref()) == Some(asked.trim());
        (asked.trim().to_owned(), landed)
    });
    Ok(opened_answer(
        &number,
        &where_it_is,
        ours,
        named
            .as_ref()
            .map(|(name, landed)| (name.as_str(), *landed)),
        &render_pane_list(&panes, Some(opener)),
    ))
}

/// [`tool_open_pane`]'s answer, as a pure function so EVERY one of its branches can be read.
///
/// Split out for the reason [`pane_summary`] is: the `ours == false` and `named = Some((_, false))`
/// branches cannot be reached against the daemon this suite builds, because a daemon that records
/// these facts always records them. They exist for a daemon at the SAME wire protocol that predates
/// the field — which accepts the argument and drops it — and that case was found by RUNNING the
/// skew proof rather than by reasoning about it. Unit-testable here; unreachable live, and recorded
/// as such.
///
/// `named` is `Some((name, landed))` when the caller asked for one: `landed` says whether the pane
/// really came back carrying it.
fn opened_answer(
    number: &str,
    where_it_is: &str,
    ours: bool,
    named: Option<(&str, bool)>,
    listing: &str,
) -> String {
    let answerable = if ours {
        "It is recorded as opened by your pane, so close_pane will let you close it."
    } else {
        "WARNING: this terminal did not record it as opened by you, so close_pane will refuse it — \
         the daemon is older than this tool. Ask the user to close it, or to restart the terminal."
    };
    // The name sentence leads with what the caller should DO with it, because a name is only worth
    // anything if it is used in place of the number — and says so ONLY when the name really landed.
    let called = match named {
        Some((name, true)) => format!(
            " It is called {name:?} — pass that as `pane` instead of the number, which will shift \
             if an earlier pane closes."
        ),
        Some((name, false)) => format!(
            " WARNING: this terminal did not record the name {name:?}, so addressing the pane by \
             it will fail — the daemon is older than this tool. Use the number, and expect it to \
             shift if an earlier pane closes."
        ),
        None => String::new(),
    };
    format!("Opened pane {number}{where_it_is}, running a shell. {answerable}{called}\n\n{listing}")
}

/// `close_pane` — end a pane THIS pane opened, refusing every other one.
///
/// # The gate is ergonomic, not a security boundary, and says so
///
/// There is no boundary to build here: the daemon's socket is local and its peers are all one
/// user's own clients, and an agent that can `write_pane` into a shell can run `sprag kill-pane`
/// itself. What the gate removes is the agent's own MISTAKE — a mis-resolved pane number ending a
/// person's editor and taking its scrollback with it, which
/// [`kill_pane`](sprag_host::HostClient::kill_pane) is explicit about being unconditional. That is
/// the failure that actually happens, and the fact it reads
/// ([`Pane::opened_by`](sprag_terminal::Pane::opened_by)) is fixed at birth, so the gate cannot be
/// acting on something that moved under it.
///
/// # One read, not two
///
/// The number is resolved and the gate is evaluated from the SAME pane listing. Reading the
/// provenance in a second query would mean the number named one pane at the first instant and the
/// gate answered about another at the second — the torn read this surface's other joins are
/// written to avoid. What can still change afterwards is whether the pane is there at all, and the
/// daemon answers that.
/// Refuse a pane this agent did not open — R294's authorship gate, in the ONE place that applies it.
///
/// # Why it is a function and not four copies
///
/// `close_pane`, `rename_pane`, `swap_pane` and `resize_pane` all ask the same question of the same
/// argument, and before R312 each asked it against a listing IT had read: the gate could only see
/// the caller's own window, so the four verbs refused a far pane with a sentence about names rather
/// than about ownership. The provenance now travels on the resolved pane
/// ([`PaneRef::info`]) from the reading that resolved it, so the gate is one rule with four callers
/// and it reaches exactly as far as the resolver does.
///
/// `verb` is the tool's own name and `consequence` the clause that says why the pane is not the
/// agent's to touch — the two halves that differ between the four, so nothing else can.
fn require_own_pane(pane: &PaneRef, verb: &str, consequence: &str) -> Result<(), String> {
    let mine = own_pane();
    match pane.info.opened_by {
        Some(opener) if Some(opener) == mine => Ok(()),
        Some(opener) => Err(format!(
            "{} was opened by {}, not by you, so {verb} will not touch it. {consequence}",
            pane.subject(),
            opener_subject(opener),
        )),
        None => Err(format!(
            "{} was opened by a person, not by you, so {verb} will not touch it. {consequence} \
             Only a pane you opened yourself with open_pane is yours.",
            pane.subject(),
        )),
    }
}

/// How a refusal names the pane that DID open the subject — by number when the caller's own window
/// holds the opener, else by id, because a number means nothing outside the listing it indexes.
///
/// It reads the listing itself, and that is safe here for the reason the whole surface now rests
/// on: what crosses between the two instants is an ID, which never moves. The number it prints is
/// the CURRENT one, which is the one the caller would type next.
fn opener_subject(opener: u64) -> String {
    let panes = query_panes().unwrap_or_default();
    short_name(PaneId(opener), &|id| {
        panes.iter().position(|p| p.id == id.0).map(numbered)
    })
}

fn tool_close_pane(args: &Value) -> Result<String, String> {
    // Through the ONE resolver, so a pane this agent opened in another window is closable by the
    // name it was opened with — and so the gate below reads the provenance of the pane the caller
    // actually named, from the reading that resolved it.
    let pane = resolve_pane_ref(args)?;
    let mine = own_pane();
    require_own_pane(
        &pane,
        "close_pane",
        "It may hold work nobody else can get back.",
    )?;
    // Whether anything is NUMBERED AFTER the pane being closed. The renumbering sentence below is a
    // claim about this run, so it is decided by what this run actually holds: closing the last pane
    // moves nothing, and telling a caller its map has shifted when it has not is the same defect as
    // staying silent when it has. A pane in ANOTHER window renumbers nothing here at all, which is
    // the third answer the sentence now has to give.
    let renumbered = match pane.number {
        Some(number) => Some(number < query_panes()?.len()),
        None => None,
    };
    let answer = host_call(
        "scene/invoke",
        with_args(
            pane_params(&pane, mux_action_path(CLOSE_ACTION)),
            json!({ "id": pane.id() }),
        ),
    )?;
    // How far the close CASCADED (R309). An agent has to be told this without being asked, because
    // it is the one outcome of this tool that changes what its OTHER tools can still reach: a
    // window that went takes its panes' numbering with it, and a session that went takes every
    // sibling pane this agent was reading. The word is the daemon's — a caller that counted the
    // listing above would be describing the state before the kill.
    //
    // ⚠ NOT REACHABLE THROUGH THIS TOOL TODAY, and that is written down rather than left for a
    // reader to work out: the gate above refuses a pane this agent did not open, and the daemon's
    // `close` acts within the scope's CURRENT window — so the agent's own pane is always a sibling
    // of the one being closed, and the answer is always `Ended::Pane`. It is read anyway because
    // the alternative is a tool that stays silent the day either of those two facts changes, which
    // is precisely the failure this round exists to fix one layer down. The `Ended::Pane` path IS
    // covered: `an_agent_closes_only_what_it_opened` pins a sentence with no clause in it.
    let beyond = Ended::from_wire(answer[ENDED_KEY].as_str().unwrap_or_default())
        .and_then(|ended| ended.beyond(Ended::Pane))
        // Worded away from the renumbering sentence below, which also says "last pane" and means
        // last by NUMBER: these are two different facts and a reader must not have to guess which.
        .map(|clause| format!("Its window held no other pane, so {clause}. "));
    Ok(format!(
        "Closed {} (id {}), which you had opened. {}{}\n\n{}",
        pane.subject(),
        pane.id(),
        beyond.unwrap_or_default(),
        match renumbered {
            Some(true) => "The panes after it have MOVED UP a number:",
            Some(false) => "It was the last pane, so the others keep their numbers:",
            // It was not in your window, so your numbering never held it.
            None => "It was in another window, so YOUR numbering is unchanged:",
        },
        // NOT `?`: the destructive part already happened. A re-read that fails here is a broken
        // connection, not a failed close, and returning an error for it would tell the caller its
        // pane is still there when it is gone — the one report that would make it act wrongly.
        // The listing is a convenience on top of the outcome; the outcome is reported either way.
        relisted(mine)
    ))
}

/// `rename_pane` — name a pane THIS pane opened, refusing every other one.
///
/// # The same gate as [`tool_close_pane`], on the same argument
///
/// A pane's name is what a PERSON reads on it (`sprag panes`, and every display surface that
/// prefers it over the child's title), so renaming somebody's pane changes what they see — which is
/// R294's own reason for gating the close, applied unchanged. No new policy is derived here, and
/// the gate is ergonomic rather than a boundary for that same entry's reason: an agent that can
/// `write_pane` into a shell can run `sprag rename-pane` itself.
///
/// It is deliberately NOT gated on the daemon side. `rename_pane` on the wire renames any pane,
/// because the CLI is an operator's and an operator means it — the daemon publishes the fact, this
/// surface applies the policy, which is the split R294 established.
///
/// # One read, not two
///
/// [`tool_close_pane`]'s rule, for its reason: the target is resolved and the gate evaluated from
/// ONE listing, so the pane the caller named and the pane the gate answered about are the same pane
/// at the same instant.
fn tool_rename_pane(args: &Value) -> Result<String, String> {
    let new = match args.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(Value::Null) | None => None,
        Some(other) => return Err(format!("'name' must be a string, not {other}")),
    };
    let pane = resolve_pane_ref(args)?;
    let subject = pane.subject();
    require_own_pane(
        &pane,
        "rename_pane",
        "Its name is what a person reads on that pane.",
    )?;
    let mut action_args = json!({ "pane": pane.id() });
    if let Some(new) = &new {
        action_args["name"] = json!(new);
    }
    // The daemon's answer carries the name it RECORDED, so this reports what landed rather than
    // what was asked for — a name is trimmed on the way in, and echoing the request would tell the
    // caller to address the pane by a string that does not resolve.
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(
            pane_params(&pane, mux_action_path(RENAME_PANE_ACTION)),
            action_args,
        ),
    )
    .map_err(|why| match &new {
        Some(new) => refusal_sentence(
            &why,
            &format!(
                "could not name {subject} {new:?}: the name may already be taken by another \
                 pane, or be blank, over 80 bytes, all digits, or contain a control character. \
                 Call list_windows to see which names are in use across the session."
            ),
        ),
        None => why.0,
    })?;
    match answer.get("name").and_then(Value::as_str) {
        Some(recorded) => Ok(format!(
            "{subject} is now called {recorded:?}. Pass that as `pane` instead of a number — a \
             name reaches any window of this session and a number never leaves yours."
        )),
        // Total over the clear AND over a daemon older than the recorded-name answer: either way
        // the pane has no name this tool can promise, which is the honest thing to say.
        None => match pane.number {
            Some(number) => Ok(format!(
                "{subject} has no name now; address it by its number ({number}), which will shift \
                 if an earlier pane closes."
            )),
            // A pane one window over has no number on this surface, so taking its name away leaves
            // NOTHING this surface can address it with. That is said, because the caller's next
            // call would otherwise fail for a reason it could not see.
            None => Ok(format!(
                "{subject} has no name now. It is in another window, so it has no number here \
                 either — nothing on this surface can address it until it is named again."
            )),
        },
    }
}

/// The pane listing for an answer whose ACTION has already happened, degrading to a sentence rather
/// than to an error.
///
/// The two structural writes both re-read to repair the caller's map of numbers. That read comes
/// AFTER the thing being reported, so its failure must not be reported as the write's failure: a
/// close that says "error" about a pane that really is closed sends the caller off to close it
/// again, or to believe a person's work survived when it did not.
fn relisted(here: Option<u64>) -> String {
    match query_panes() {
        Ok(panes) => render_pane_list(&panes, here),
        Err(why) => format!("(could not re-list the panes: {why} — call list_panes)"),
    }
}

/// `select_pane` — move the SESSION's active pane, which every attached client follows.
///
/// The answer names what actually happened rather than echoing the request: the daemon reports
/// whether the pane MOVED, and a re-select of the pane the session is already on is a legitimate
/// no-op an agent should not read as a failure. (These three sentences sat above
/// [`tool_open_pane`] for four rounds, glued to the front of ITS doc by a missing blank line — so
/// this function had none at all and that one opened by describing a different verb.)
///
/// # Why a DIRECTION belongs on an agent's surface
///
/// Because without it an agent cannot ask for one at all without joining two instants.
/// [`tool_pane_layout`] publishes the daemon's own adjacency and this tool took a NUMBER, so "put
/// them on the pane to the left" was a layout read at one moment and a select at another — the torn
/// read a pane NAME exists to prevent, rebuilt out of two correct tools. The wire arm has resolved
/// directions under one lock since the placement verbs shipped; the only thing missing was this
/// argument.
///
/// It also settles a question R285 answered the other way for the zoom (*an agent reads and types; a
/// zoom is a thing you do FOR a human to look at*). A directional SELECT is not the zoom's case:
/// this tool's whole subject is already the person, so the argument that declines an arrangement
/// verb does not reach it. What an agent still cannot do is move a pane — that decision stays a
/// person's.
///
/// # Whose position a direction is relative to, said out loud
///
/// The ACTIVE pane's by default — where the user is — which is the daemon's own default and the
/// same semantics as the keybinding and the CLI verb, so one vocabulary means one thing on all three
/// surfaces. `from` and `from_here` are how a caller asks the OTHER question, and they exist because
/// this surface's caller is the one that most often means it: an agent lives in a pane and reasons
/// about the panes around ITS OWN, where a keypress can only ever mean "from here".
///
/// # Why `from_here` is a separate argument and not a value of `from`
///
/// Because `from` carries this surface's pane handles, and both of them are already taken: a NUMBER
/// is a position in `list_panes` and a STRING is a pane's name. A magic word like `"self"` would
/// collide with a pane somebody named `self` — the exact ambiguity R295 forbade all-digit names to
/// avoid — and resolving it "as a name first, then as the sentinel" is a silent wrong answer waiting
/// for the day the name exists. A boolean cannot collide with either.
///
/// It is also the argument that costs NOTHING to answer: [`own_pane`] reads this process's own
/// environment, so "the pane next to mine" is one call with no listing at all, where a number would
/// have to be looked up first and could name a different pane by the time it is sent.
fn tool_select_pane(args: &Value) -> Result<String, String> {
    // Exactly one naming, the wire action's own rule — restated here because the daemon can only
    // answer `Rejected` for a malformed one (`InvokeError::Rejected` carries no payload, upstream
    // PINION-PR82), and "select nothing" / "select two things" have no obvious reading to guess.
    let toward = match args.get("dir") {
        None | Some(Value::Null) => None,
        Some(Value::String(word)) => Some(PaneDir::from_wire(word).ok_or_else(|| {
            format!(
                "'dir' must be one of {}, not {word:?}",
                PaneDir::ALL.map(PaneDir::wire_str).join(", ")
            )
        })?),
        Some(other) => return Err(format!("'dir' must be a direction word, not {other}")),
    };
    let named = args
        .get(SelectAsk::PANE_KEY)
        .is_some_and(|pane| !pane.is_null());
    // Read before the arms so a `from` handed to the `pane` arm is a REFUSAL rather than a silently
    // ignored argument — the failure R294 measured an old daemon making, which this surface must not
    // re-make one layer up.
    let origin = if toward.is_some() {
        select_origin(args)?
    } else {
        forbid_origin(args)?;
        None
    };
    // The fourth member is the WINDOW the request must be narrowed to. Measured at `e7be5eb`:
    // `select_pane` resolves against the SCOPE's window, so a bare request naming a pane one window
    // over is `Rejected` — and the same request with `window=` answers `already_active`. It sets
    // THAT window's active pane and leaves the user's current window alone, which is the honest
    // meaning of selecting a pane you are not looking at.
    let (action_args, asked, subject, window) = match (named, toward) {
        (true, None) => {
            // ONE listing, and it serves both halves: it resolves the caller's number-or-name AND
            // names the pane the answer will be about. A `pane` request can only land on the pane it
            // named or be refused, so there is nothing to re-read — reading again would name the
            // subject at a second instant for no fact gained (the discipline [`resolve_pane`]
            // documents, which this function cannot use because it needs the pane's NAME too).
            let pane = resolve_pane_ref(args)?;
            (
                json!({ "pane": pane.id() }),
                None,
                Some(render_pane_handle(&pane)),
                pane.window.clone(),
            )
        }
        (false, Some(dir)) => (
            SelectAsk::Toward {
                dir,
                from: origin.as_ref().map(|(pane, _)| PaneId(pane.id())),
            }
            .to_args(),
            Some(dir),
            None,
            // A direction is walked WITHIN a window, so the origin is what says which one — and
            // with no origin it is the caller's own, exactly as before.
            origin.as_ref().and_then(|(pane, _)| pane.window.clone()),
        ),
        (false, None) => {
            return Err(
                "select_pane needs either 'pane' (a NUMBER from list_panes, or a pane's NAME) or \
                 'dir' (\"left\" / \"right\" / \"up\" / \"down\", one step from where the user \
                 is — or from the pane you name with 'from' / 'from_here')"
                    .to_owned(),
            );
        }
        (true, Some(_)) => {
            return Err(
                "'pane' and 'dir' name the target two different ways; give one. 'pane' selects \
                 THAT pane; 'dir' moves one pane that way. To step from a pane you choose, keep \
                 'dir' and name it with 'from' instead of 'pane'."
                    .to_owned(),
            );
        }
    };
    let answer = host_call(
        "scene/invoke",
        with_args(
            windowed_params(mux_action_path(SELECT_PANE_ACTION), window.as_deref()),
            action_args,
        ),
    )?;
    let how = SelectHow::read(&answer, asked);
    // The daemon answers with an ID; this surface speaks NUMBERS and names. A DIRECTION is the one
    // arm whose caller cannot know where it landed, so that one resolves the id from a listing read
    // AFTER the action — the answer describes the state the action left. Its failure is NOT the
    // call's failure ([`relisted`]'s rule): the user's cursor has already moved, and an "error" would
    // send the caller to move it again.
    let landed = answer["pane"].as_u64();
    let here = subject.or_else(|| {
        let panes = query_panes().ok()?;
        let index = panes.iter().position(|pane| Some(pane.id) == landed)?;
        Some(render_pane_handle(&PaneRef {
            number: Some(numbered(index)),
            window: None,
            info: panes[index].clone(),
        }))
    });
    let current = current_window_name();
    Ok(render_selection(
        how,
        asked,
        origin.as_ref().map(|(_, label)| label.as_str()),
        here.as_deref(),
        landed.unwrap_or_default(),
        window
            .as_deref()
            .zip(current.as_deref())
            .filter(|(window, current)| window != current),
    ))
}

/// `swap_pane` — move a pane THIS pane opened to a different place in the arrangement.
///
/// # Why an agent may write the arrangement at all, when R294 said it may not
///
/// R294 re-derived debt item 14 and answered **NO to `move`/`swap`/`zoom`** on R285's argument:
/// *those decide what a HUMAN looks at, and an agent has no basis for the decision*. That argument
/// is not inverted here. **Its premise moved, in the same round that stated it**: R294's own YES
/// half was *"opening and closing its own workbench"*, and it created the first panes an agent is
/// answerable for ([`PaneInfo::opened_by`]). For a pane a person opened an agent still has no basis
/// — they arranged it. For a pane the agent opened it is the only party that has one.
///
/// So the gate is authorship, which is [`tool_close_pane`]'s and [`tool_rename_pane`]'s gate on the
/// same argument — the THIRD instance of one policy rather than a new one — and it is ergonomic
/// rather than a boundary for R294's stated reason: an agent that can `write_pane` into a shell can
/// run `sprag swap-pane` itself. What it removes is the agent's own mistake.
///
/// The DAEMON is deliberately ungated: `sprag swap-pane` is an operator's verb and an operator means
/// it. The daemon publishes the fact, this surface applies the policy — R294's split.
///
/// # Why `pane` is required, where `select_pane`'s is optional
///
/// Because its default would be the ACTIVE pane, which is the pane a person is typing in, which the
/// gate above refuses by construction. An argument whose default can only fail is not a default. The
/// same reasoning removes the `_here` spelling this tool's twin has: the pane this server runs in was
/// opened by a person and handed to the agent, so `from_here`'s counterpart could never be accepted
/// either. Both absences are decisions, and this is where they are written down.
///
/// # One read, not two
///
/// [`tool_close_pane`]'s rule: the target is resolved and the gate evaluated from ONE listing, so
/// the pane the caller named and the pane the gate answered about are the same pane at the same
/// instant. The PARTNER is resolved from that same listing for the same reason.
fn tool_swap_pane(args: &Value) -> Result<String, String> {
    let toward = match args.get(SwapAsk::DIR_KEY) {
        None | Some(Value::Null) => None,
        Some(Value::String(word)) => Some(PaneDir::from_wire(word).ok_or_else(|| {
            format!(
                "'{}' must be one of {}, not {word:?}",
                SwapAsk::DIR_KEY,
                PaneDir::ALL.map(PaneDir::wire_str).join(", ")
            )
        })?),
        Some(other) => {
            return Err(format!(
                "'{}' must be a direction word, not {other}",
                SwapAsk::DIR_KEY
            ));
        }
    };
    let named = args
        .get(SwapAsk::WITH_KEY)
        .is_some_and(|with| !with.is_null());
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    require_own_pane(
        &pane,
        "swap_pane",
        "Where their panes sit is their arrangement. (To put the user ON a pane instead of moving \
         one, use select_pane.)",
    )?;
    // Exactly one partner, the wire action's own rule — restated here because the daemon can only
    // answer `Rejected` for a malformed request (`InvokeError::Rejected` carries no payload,
    // upstream PINION-PR82), and neither mistake has a reading worth guessing.
    let (ask, partner) = match (named, toward) {
        (true, None) => {
            let with = resolve_pane_ref_at(args, SwapAsk::WITH_KEY)?;
            (
                SwapAsk::With {
                    pane: Some(PaneId(pane.id())),
                    with: PaneId(with.id()),
                },
                Some(render_pane_handle(&with)),
            )
        }
        (false, Some(dir)) => (
            SwapAsk::Toward {
                pane: Some(PaneId(pane.id())),
                dir,
            },
            None,
        ),
        (false, None) => {
            return Err(format!(
                "swap_pane needs either '{}' (a NUMBER from list_panes, or a pane's NAME — trade \
                 with THAT pane) or '{}' (\"left\" / \"right\" / \"up\" / \"down\" — trade with \
                 the pane one step that way)",
                SwapAsk::WITH_KEY,
                SwapAsk::DIR_KEY,
            ));
        }
        (true, Some(_)) => {
            return Err(format!(
                "'{}' and '{}' name the partner two different ways; give one. '{}' trades with \
                 THAT pane; '{}' trades with whatever is one step that way.",
                SwapAsk::WITH_KEY,
                SwapAsk::DIR_KEY,
                SwapAsk::WITH_KEY,
                SwapAsk::DIR_KEY,
            ));
        }
    };
    let answer = host_call(
        "scene/invoke",
        // No window narrowing, and that is measured rather than assumed: `swap_pane` resolves
        // REGISTRY-wide at the daemon, so it swaps a pane one window over with no scope help. The
        // directional arm walks the SUBJECT's own window, which is where the subject is.
        with_args(
            pane_params(&pane, mux_action_path(SWAP_PANE_ACTION)),
            ask.to_args(),
        ),
    )?;
    let how = SwapHow::read(&answer, toward);
    // The daemon answers with IDS; this surface speaks numbers and names. A DIRECTION is the arm
    // whose caller cannot know who it traded with, so that one resolves the partner from a listing
    // read AFTER the action — the answer describes the state the action left. Its failure is NOT
    // the call's failure (`relisted`'s rule): the arrangement has already moved.
    // The `dir` arm's partner is an ID the caller never named, and it is resolved against the
    // listing ALREADY IN HAND rather than a fresh one — which is not a shortcut but the honest
    // reading. A swap moves no pane's NUMBER: numbers are pool order, and this verb changes only
    // where the panes sit, which is the whole `panes`-answers-WHO / `layout`-answers-WHERE split.
    // Re-reading would name the partner at a second instant for no fact gained, and cost a third
    // host call — the two-instant join the directional arm exists to remove. (`close_pane` DOES
    // re-read, because a close is the one verb that renumbers.)
    let partner = partner.or_else(|| {
        let with = answer["b"].as_u64()?;
        // The subject's OWN window is the one a direction walks, so that is the listing the
        // partner is named from — and a partner of a far pane gets the same handle its subject
        // did, an id and a name, because neither has a number on this surface.
        let sibling = match &pane.window {
            Some(window) => query_window_panes(window).ok()?,
            None => query_panes().ok()?,
        };
        let index = sibling.iter().position(|row| row.id == with)?;
        Some(render_pane_handle(&PaneRef {
            number: pane.window.is_none().then(|| numbered(index)),
            window: pane.window.clone(),
            info: sibling[index].clone(),
        }))
    });
    Ok(render_swap(how, toward, &subject, partner.as_deref()))
}

/// What `swap_pane` tells the agent, as a pure function of the outcome — [`render_selection`]'s rule,
/// so all four sentences are pinned by unit tests and not only the ones a live daemon can be driven
/// into.
///
/// `subject` is the pane the caller asked to move, in this surface's own vocabulary, and it is the
/// subject of every sentence including the two where nothing happened — unlike the select, where a
/// step that goes nowhere leaves the user on a pane that may not be the origin. Here there is no
/// third pane to confuse: a swap that traded nothing left the named pane exactly where it was.
///
/// The two nothing-happened outcomes get distinct sentences with distinct remedies, which is the
/// whole point of the daemon naming them: an edge means "look the other way", a floating pane means
/// "there is no way to look at all".
/// `resize_pane` — move the boundary beside a pane THIS SERVER OPENED, in cells.
///
/// [`tool_swap_pane`]'s ownership gate, unchanged and for its reason: a resize necessarily takes
/// cells FROM the pane on the other side of the boundary, so an agent widening somebody's pane
/// narrows another one they did not ask about. The rival's `pane.resize` resizes any pane by id
/// (herdr `9a4ce5e1`, `handle_pane_resize`) — nothing on one of their panes records what created
/// it, so there is no gate to apply.
///
/// The argument for the verb EXISTING on this surface is its own, and it is not the swap's. R285
/// declined an MCP zoom because *"an agent reads and types; a zoom is a thing you do FOR a human to
/// look at"*, and R288 recorded that the argument had inverted once already. It inverts here too, in
/// a way neither of those verbs does: **a pane's WIDTH is what decides whether the output this
/// server itself reads is wrapped**, so a resize is the one arrangement verb whose subject is the
/// agent's own reading.
fn tool_resize_pane(args: &Value) -> Result<String, String> {
    let dir = match args.get(ResizeAsk::DIR_KEY) {
        Some(Value::String(word)) => PaneDir::from_wire(word).ok_or_else(|| {
            format!(
                "'{}' must be one of {}, not {word:?}",
                ResizeAsk::DIR_KEY,
                PaneDir::ALL.map(PaneDir::wire_str).join(", ")
            )
        })?,
        _ => {
            return Err(format!(
                "resize_pane needs '{}' (\"left\" / \"right\" / \"up\" / \"down\") — which way the \
                 BOUNDARY beside the pane moves",
                ResizeAsk::DIR_KEY,
            ));
        }
    };
    let cells = match args.get(ResizeAsk::CELLS_KEY) {
        None | Some(Value::Null) => ResizeAsk::CELLS_DEFAULT,
        Some(value) => value
            .as_u64()
            .and_then(|cells| u16::try_from(cells).ok())
            .filter(|cells| *cells > 0)
            .ok_or_else(|| {
                format!(
                    "'{}' must be a whole number of cells, 1 or more — {value} is not a distance",
                    ResizeAsk::CELLS_KEY,
                )
            })?,
    };
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    require_own_pane(
        &pane,
        "resize_pane",
        "How big their panes are is their arrangement.",
    )?;
    let ask = ResizeAsk {
        pane: Some(PaneId(pane.id())),
        dir,
        cells,
    };
    // Through `host_call_kinded`, because this action REFUSES and the daemon can only say
    // `Rejected` (upstream PINION-PR82). ⚠ Until R313's audit that refusal reached the agent as
    // `host rpc error: InvokeRejected` — a Rust variant name, which is debt item 9's class, and it
    // was unreachable before only because the authorship gate refused first: the moment an agent
    // owned a pane in a window nobody is watching, the leak was the answer it got.
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(
            pane_params(&pane, mux_action_path(RESIZE_PANE_ACTION)),
            ask.to_args(),
        ),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "{subject} could not be resized: it is floating, or it has no boundary {}, or \
                 nothing is WATCHING its window — a cell has no length until a client reports an \
                 area, so a window nobody has looked at yet cannot have its dividers moved.",
                dir.wire_str(),
            ),
        )
    })?;
    let how = ResizeHow::from_wire(answer[OUTCOME_KEY].as_str().unwrap_or_default())
        .unwrap_or(ResizeHow::Resized);
    let moved = u16::try_from(answer["cells"].as_u64().unwrap_or_default()).unwrap_or(u16::MAX);
    Ok(render_resize(how, ask, &subject, moved))
}

/// What `resize_pane` tells the agent, as a pure function of the daemon's answer.
///
/// The nothing-happened halves come from [`ResizeHow::why`], which the CLI verb also reads — one
/// wording for two surfaces — with this surface adding what an AGENT does next. The CLAMPED case is
/// the one worth spelling out here: it is not an outcome word, and an agent that asked for twenty
/// columns and got seven has to know that asking again will get it nothing.
fn render_resize(how: ResizeHow, ask: ResizeAsk, subject: &str, moved: u16) -> String {
    match how.why(ask.dir) {
        Some(why) => format!("{subject} was not resized: {why}."),
        None if moved < ask.cells => format!(
            "Moved {subject}'s {} boundary {moved} cell{} of the {} asked for — it reached the \
             last cell the far side may keep, so there is no more room that way. Call read_pane to \
             see the pane at its new width.",
            ask.dir.wire_str(),
            if moved == 1 { "" } else { "s" },
            ask.cells,
        ),
        None => format!(
            "Moved {subject}'s {} boundary {moved} cell{}; the pane on the other side of it gave \
             up exactly that much. Call read_pane to see the pane at its new width.",
            ask.dir.wire_str(),
            if moved == 1 { "" } else { "s" },
        ),
    }
}

/// `zoom_pane` — fill a window with a pane THIS SERVER OPENED, or put the arrangement back.
///
/// # Why this verb exists here at all, when R285 said no
///
/// R285 declined an MCP zoom because *"an agent reads and types; a zoom is a thing you do FOR a
/// human to look at"*, and R294 re-derived the same NO for `move`/`swap`/`zoom`. Two of those three
/// have since been built on an argument that inverts the premise rather than the conclusion —
/// [`tool_swap_pane`]'s authorship gate, and [`tool_resize_pane`]'s *"a pane's WIDTH is what decides
/// whether the output this server reads is wrapped"*. The zoom is the SAME argument as the resize's,
/// at its limit: a zoomed pane is given the whole window, so it is the widest this server can make
/// anything it reads. That the person also sees it is true of `select_pane` and `display_message`
/// too; what makes it acceptable is the gate, not the invisibility.
///
/// # `pane` is REQUIRED, where the wire action's is optional
///
/// [`tool_swap_pane`]'s rule for its reason: the wire's default is the ACTIVE pane, which is the
/// pane a person is typing in, which the gate below refuses by construction. An argument whose
/// default can only fail is not a default.
fn tool_zoom_pane(args: &Value) -> Result<String, String> {
    let on = match args.get("on") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(other) => {
            return Err(format!(
                "'on' must be true (fill the window) or false (put the arrangement back), not \
                 {other} — omit it to toggle"
            ));
        }
    };
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    require_own_pane(
        &pane,
        "zoom_pane",
        "Which pane fills their window decides what they can see.",
    )?;
    let mut ask = json!({ "pane": pane.id() });
    if let Some(on) = on {
        ask["on"] = json!(on);
    }
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(pane_params(&pane, mux_action_path(ZOOM_PANE_ACTION)), ask),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "{subject} could not be zoomed: it is floating, so its window does not tile it and \
                 there is no arrangement for it to fill."
            ),
        )
    })?;
    // FOUR distinct outcomes, and no arm consults what this process ASKED for — the CLI verb's
    // rule, for its reason: the daemon REFUSES a target it cannot zoom rather than answering one of
    // these about it, so each pair means exactly one thing.
    Ok(render_zoom(
        &subject,
        answer["zoomed"].as_bool().unwrap_or(false),
        answer["changed"].as_bool().unwrap_or(false),
    ))
}

/// What `zoom_pane` tells the agent, as a pure function of the daemon's answer —
/// [`render_swap`]'s rule, so all four sentences are pinned by a unit test
/// (`the_four_zoom_sentences_each_say_which_state_they_left`) and not only by what a live daemon
/// can be driven into.
///
/// Each says what to do NEXT, because that is the half an agent cannot infer: a pane that now fills
/// its window is a pane whose width just changed, and the reason to zoom on this surface is to read
/// it at that width.
fn render_zoom(subject: &str, zoomed: bool, changed: bool) -> String {
    match (zoomed, changed) {
        (true, true) => format!(
            "{subject} now fills its window and is the pane its window is on. read_pane sees it at \
             the window's full width — call zoom_pane with `on: false` when you are done, because \
             anybody looking at that window sees only this pane until then."
        ),
        (true, false) => format!(
            "{subject} was already filling its window; nothing moved. read_pane already sees it at \
             the window's full width."
        ),
        (false, true) => format!(
            "{subject} no longer fills its window — the arrangement is back, and every pane in that \
             window is visible again. read_pane sees {subject} at its tiled width now."
        ),
        (false, false) => format!(
            "{subject} was not filling its window, so the arrangement was already showing; nothing \
             moved."
        ),
    }
}

/// `break_pane` — take a pane THIS SERVER OPENED out into a window of its own.
///
/// # The window it makes is [`tool_open_window`]'s window
///
/// Born DETACHED and recorded as opened by this pane — the same two facts, sent through the same
/// [`WindowBirthAsk`] grammar. That is the whole reason this tool could not simply wrap the wire
/// action: before R335 a break took the screen and claimed nobody, so an agent tidying its own pane
/// out of somebody's window moved every attached client and then could not close what it had made.
/// A tool is not the place to paper over either — the daemon says how a window is born.
///
/// # Why the gate is on the PANE and there is none on the window
///
/// Because the window does not exist yet. [`require_own_pane`] is the whole authorisation: an agent
/// may move what it opened, and what it opens is answerable to it.
///
/// # The rivals, measured — and the honest trade first
///
/// **herdr is AHEAD on breadth and on one join sprag did not have.** Its programmatic surface is 91
/// methods (`src/api/schema.rs:45`, `Method`) against this roster's 33, and its CLI is BUILT on that
/// API — `src/cli/pane.rs`, `tab.rs`, `worktree.rs` and seven more all `use crate::api::schema` — so
/// its shell and its API cannot drift the way sprag's shell and agent surface had. `pane.move`,
/// `pane.swap` and `pane.zoom` have been on that surface while sprag's agent had none of the three.
///
/// What neither rival has, re-measured at herdr `9a4ce5e1` and ghostty `2602886`:
///
/// * **A pane can LEAVE its window and come back.** No method among herdr's 91 is a break or a join
///   (nothing named `break`/`join`/`detach`/`extract` in `Method`). ghostty's nearest is
///   `move_tab_to_new_window` (`src/input/Binding.zig:594`) — a TAB, not a split — and no action of
///   its 200-odd moves a SPLIT out of its window or into another at all.
/// * **One request places a pane whether or not it crosses a window.** herdr needs two methods and
///   leaves a hole between them: `pane.swap` refuses across a tab (`PaneSwapReason::CrossTab`,
///   `src/app/api/panes.rs:533`) and `pane.move` refuses to stay inside one
///   (`PaneMoveReason::SameTab`, `:699`), so moving a pane WITHIN its own tab is expressible in
///   neither.
/// * **A zoom does not block the arrangement.** herdr refuses a move into or out of a zoomed tab
///   (`PaneMoveReason::ZoomedTab`, `:665` and `:732`); sprag's zoom is a filter on the projection,
///   so [`tool_move_pane`], `swap` and `set_layout` all still serve.
/// * **Authorship.** Every agent write here is gated on who opened the pane. herdr records nothing
///   about what created one — no `opened_by` anywhere in its tree — so the gate has nothing to read.
/// * **The keyboard is in the join too.** herdr's bindings are a config struct
///   (`src/config/keybinds.rs`), not a projection of `Method`, and nothing holds the two together.
fn tool_break_pane(args: &Value) -> Result<String, String> {
    let opener = own_pane().ok_or(
        "break_pane needs to know which pane you are running in, and this server is not inside one \
         (no SPRAG_PANE published beside the socket it is talking to). Without it the daemon \
         cannot record the new window as yours, and close_window would then refuse it.",
    )?;
    let name = match args.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(Value::Null) | None => None,
        Some(other) => return Err(format!("'name' must be a string, not {other}")),
    };
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    require_own_pane(
        &pane,
        "break_pane",
        "Where their panes sit is their arrangement.",
    )?;
    let mut ask = Value::Object(
        WindowBirthAsk(sprag_terminal::WindowBirth {
            detached: true,
            opened_by: Some(PaneId(opener)),
        })
        .to_args(),
    );
    ask["pane"] = json!(pane.id());
    if let Some(name) = &name {
        ask["name"] = json!(name);
    }
    let created = host_call_kinded(
        "scene/invoke",
        with_args(pane_params(&pane, mux_action_path(BREAK_PANE_ACTION)), ask),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "{subject} could not be broken out: it may be the only pane its window tiles — \
                 which would be a rename dressed as a move, so use rename_window instead{}",
                match &name {
                    Some(name) => format!(" — or the name {name:?} may already be taken."),
                    None => ".".to_owned(),
                },
            ),
        )
    })?;
    let created = created
        .as_str()
        .ok_or("the host did not answer with the new window's name")?;
    // WHETHER THE PROVENANCE LANDED, read back off the window — [`tool_open_window`]'s rule and for
    // its reason: a daemon that predates the key accepts the request and records nothing, and
    // promising a close this surface would then refuse is the one report that makes a caller act
    // wrongly. NOT `?`: the pane has already moved, and a failed re-read is not a failed break.
    let ours = query_windows()
        .ok()
        .and_then(|windows| {
            windows
                .into_iter()
                .find(|window| window.name == created)
                .map(|window| window.opened_by == Some(PaneId(opener)))
        })
        .unwrap_or(false);
    Ok(format!(
        "Moved {subject} into a window of its own, called {created}. It kept everything — its \
         contents, its scrollback and whatever is running in it. The user did NOT move and cannot \
         see it: call select_window {{\"window\": {created:?}}} when you have something for them. \
         {}\n\n{}",
        if ours {
            "It is yours to close_window and rename_window."
        } else {
            "WARNING: this terminal did not record the window as opened by you, so close_window \
             will refuse it — the daemon predates the break's window provenance."
        },
        relisted_windows(),
    ))
}

/// `join_pane` — move a pane THIS SERVER OPENED into another window of the session.
///
/// # Why the DESTINATION is ungated where the pane is not
///
/// [`tool_swap_pane`]'s split, one level up: the gate is on the pane being MOVED, because that is
/// the thing this agent is deciding about. A destination window is displaced by the arrival, not
/// moved by a decision — the same reading `swap_pane`'s partner has, and the same reading
/// `open_pane` already relies on, since an agent's first pane is opened into a window a person
/// made. Refusing a person's window here would leave an agent able to put a pane beside somebody
/// (`open_pane`) and unable to move one there, which is a rule nobody could state.
fn tool_join_pane(args: &Value) -> Result<String, String> {
    // Resolved BEFORE the request so an unknown name is a sentence naming what exists, where the
    // daemon can only answer a payload-less `Rejected` (upstream PINION-PR82).
    let window = resolve_window(args, WindowRef::WINDOW_KEY)?;
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    require_own_pane(
        &pane,
        "join_pane",
        "Where their panes sit is their arrangement.",
    )?;
    // BY IDENTITY, not by the name this tool just resolved. R330's rule at the surface that reads a
    // list and then acts on it: an agent that called list_windows and then joined means the window
    // it read about, and a name re-resolved at the daemon lands wherever that name has got to. A
    // daemon that publishes no identity gets the NAME — the reading it can honour, and the only one
    // it has — which is `tool_close_window`'s fallback verbatim.
    let ask = JoinAsk {
        pane: PaneId(pane.id()),
        window: match window.id {
            Some(id) => WindowRef::Picked(id),
            None => WindowRef::Named(window.name.clone()),
        },
    };
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(
            pane_params(&pane, mux_action_path(JOIN_PANE_ACTION)),
            ask.to_args(),
        ),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "{subject} could not be joined into window {}: it may already live there, which is \
                 a move with nowhere to go.",
                window.name,
            ),
        )
    })?;
    Ok(format!(
        "Moved {subject} into window {}, beside what was already there. It kept its contents, its \
         scrollback and whatever is running in it.{} Call list_windows to see where things are \
         now; a pane NUMBER means the Nth pane of YOUR window, so address this one by name.",
        window.name,
        if answer["closed_source"].as_bool().unwrap_or(false) {
            " The window it came FROM held nothing else, so that window closed."
        } else {
            ""
        },
    ))
}

/// `move_pane` — place a pane THIS SERVER OPENED on a chosen SIDE of a particular pane.
///
/// [`tool_join_pane`] with a PLACE, and the same gate for the same reason: the moved pane is the
/// agent's, the TARGET may be anybody's because it is divided rather than moved. One request covers
/// a re-placement inside one window and a move into another, because a
/// [`PaneId`] implies its window at both ends — see
/// [`MOVE_PANE_ACTION`], where the rival needs two methods and
/// still leaves a hole between them.
///
/// # ONE DIRECTION VOCABULARY ON THIS SURFACE, where the wire has two
///
/// The action takes tmux's split grammar — an AXIS (`horizontal` / `vertical`) plus a `before`
/// boolean — because [`SPLIT_ACTION`](sprag_host::wire::SPLIT_ACTION) does and one vocabulary spans
/// placing a new pane and placing an existing one. That is right for the wire and wrong for this
/// surface, where `dir` already means a compass direction at `select_pane`, `swap_pane` and
/// `resize_pane`: an argument spelled the same and read differently is the drift this project's
/// one-spelling rule exists to stop, and a caller cannot discover which sense it got — both words
/// parse, and `horizontal` alone does not say WHICH side.
///
/// So this tool takes [`PaneDir`]'s four words and DERIVES the pair, through the type's own
/// [`axis`](PaneDir::axis) and [`side`](PaneDir::side). Nothing is hand-mapped, the `before`
/// argument disappears (a side is one fact, not two), and the surface has one direction vocabulary
/// end to end.
fn tool_move_pane(args: &Value) -> Result<String, String> {
    let dir = match args.get("dir") {
        Some(Value::String(word)) => PaneDir::from_wire(word).ok_or_else(|| {
            format!(
                "'dir' must be one of {}, not {word:?} — it is the SIDE of the target the pane \
                 lands on",
                PaneDir::ALL.map(PaneDir::wire_str).join(", "),
            )
        })?,
        Some(other) => {
            return Err(format!("'dir' must be a direction word, not {other}"));
        }
        None => {
            return Err(format!(
                "move_pane needs 'dir' — one of {}, the SIDE of the target the pane lands on. Use \
                 join_pane to append into a window without saying where.",
                PaneDir::ALL.map(PaneDir::wire_str).join(" / "),
            ));
        }
    };
    let target = resolve_pane_ref_at(args, "target")?;
    let pane = resolve_pane_ref(args)?;
    let subject = render_pane_handle(&pane);
    let beside = render_pane_handle(&target);
    require_own_pane(
        &pane,
        "move_pane",
        "Where their panes sit is their arrangement.",
    )?;
    let answer = host_call_kinded(
        "scene/invoke",
        with_args(
            pane_params(&pane, mux_action_path(MOVE_PANE_ACTION)),
            json!({
                "pane": pane.id(),
                "target": target.id(),
                // DERIVED from the one direction the caller gave, by the type that owns both facts
                // — so a fifth direction, or a change to what a side means, cannot be half-applied
                // here.
                "dir": match dir.axis() {
                    SplitDir::Horizontal => "horizontal",
                    SplitDir::Vertical => "vertical",
                },
                "before": dir.side() == SplitSide::First,
            }),
        ),
    )
    .map_err(|why| {
        refusal_sentence(
            &why,
            &format!(
                "{subject} could not be placed beside {beside}: they may be the SAME pane, which \
                 has no reading at all, or {beside} may be floating rather than tiled where it \
                 lives, so there is no leaf to divide."
            ),
        )
    })?;
    Ok(format!(
        "Placed {subject} {} {beside}.{} Call pane_layout to see the arrangement it landed in.",
        dir.beyond(),
        if answer["closed_source"].as_bool().unwrap_or(false) {
            " The window it came FROM held nothing else, so that window closed."
        } else {
            ""
        },
    ))
}

fn render_swap(
    how: SwapHow,
    asked: Option<PaneDir>,
    subject: &str,
    partner: Option<&str>,
) -> String {
    let partner = partner.map_or_else(|| "the other pane".to_owned(), str::to_owned);
    match (how, asked) {
        (SwapHow::Swapped, Some(dir)) => format!(
            "Moved {subject} one place {}: it and {partner} have traded places. Nobody's cursor \
             moved — call select_pane if you want the user to look at it.",
            dir.wire_str()
        ),
        (SwapHow::Swapped, None) => format!(
            "{subject} and {partner} have traded places. Nobody's cursor moved — call select_pane \
             if you want the user to look at it."
        ),
        (SwapHow::AtEdge, Some(dir)) => format!(
            "There is nothing {} {subject}, so it stayed where it is: that is the edge of the \
             window. Call pane_layout to see what lies where.",
            dir.beyond()
        ),
        (SwapHow::Untiled, _) => format!(
            "{subject} is FLOATING: a floating pane is in no arrangement, so it has no neighbour in \
             any direction and nothing moved. Name the pane to trade with using 'with' instead, or \
             ask the user to dock it."
        ),
        // A pane traded with itself, and — degrading rather than failing — a daemon answering a
        // word its request could not produce. Both are honestly "nothing moved".
        (SwapHow::SamePane | SwapHow::AtEdge, _) => {
            format!("{subject} is the pane you asked to trade it with, so nothing moved.")
        }
    }
}

/// Which pane a `dir` step starts at, and how to SAY it — [`None`] for the default, the pane the
/// user is on.
///
/// Two spellings because they are two different questions, not two syntaxes for one: `from` names a
/// pane the caller picked out of the terminal, and `from_here` names the pane this server is running
/// in. Only the first needs a listing; the second is our own identity ([`own_pane`]), which is why
/// it is exact — a number looked up in one call can name a different pane by the next one, and this
/// tool MOVES A PERSON'S CURSOR on the strength of it.
fn select_origin(args: &Value) -> Result<Option<(PaneRef, String)>, String> {
    let named = args
        .get(SelectAsk::FROM_KEY)
        .filter(|value| !value.is_null());
    match (named, asks_for_here(args)?) {
        (None, false) => Ok(None),
        (Some(_), true) => Err(format!(
            "'{}' and '{FROM_HERE_ARG}' both say where to step FROM; give one. '{}' names any \
             pane; '{FROM_HERE_ARG}: true' is the pane you are running in.",
            SelectAsk::FROM_KEY,
            SelectAsk::FROM_KEY,
        )),
        (Some(_), false) => {
            let pane = resolve_pane_ref_at(args, SelectAsk::FROM_KEY)?;
            let label = render_pane_handle(&pane);
            Ok(Some((pane, label)))
        }
        (None, true) => own_pane_ref()
            .map(|pane| Some((pane, "the pane you are running in".to_owned())))
            .ok_or_else(|| {
                format!(
                    "'{FROM_HERE_ARG}' means the pane THIS server runs in, and it is not running \
                     inside a sprag pane (no {PANE_ENV_VAR} published beside the socket it is \
                     talking to). Name the pane to step from with '{}' instead.",
                    SelectAsk::FROM_KEY,
                )
            }),
    }
}

/// Whether the caller asked to step from THIS server's own pane — the one reading of
/// [`FROM_HERE_ARG`], so every gate that consults it agrees.
///
/// **`false` is ABSENT, not present.** It is the default of a boolean, so a client that fills every
/// field of an argument struct in sends it while asking for nothing — and the first draft of this
/// round refused `{dir, from: 2, from_here: false}` as "two origins" and `{pane: 1, from_here:
/// false}` as "an origin with no direction", both of which have one obvious reading. That is
/// [`SelectAsk::parse`]'s own rule about an explicit `null`, written into the wire grammar in this
/// round and then contradicted one layer up in the same round.
///
/// # Errors
///
/// A value that is neither boolean nor null, which has no reading at all.
fn asks_for_here(args: &Value) -> Result<bool, String> {
    match args.get(FROM_HERE_ARG) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(here)) => Ok(*here),
        Some(other) => Err(format!(
            "'{FROM_HERE_ARG}' must be true or false, not {other}"
        )),
    }
}

/// The `select_pane` argument that steps from the pane this server runs in — see [`select_origin`]
/// for why it is a boolean of its own rather than a value of [`SelectAsk::FROM_KEY`].
const FROM_HERE_ARG: &str = "from_here";

/// Refuse an origin on a request that does not STEP — there is nothing for it to be the origin of.
///
/// The alternative is to ignore it, and ignoring an argument is how a caller's misunderstanding
/// survives: `{pane: 3, from: 5}` from an agent that meant "left of 5" would select 3 and report
/// success. One sentence, naming what each argument does, costs a line here and saves that.
fn forbid_origin(args: &Value) -> Result<(), String> {
    let asked = [
        (
            SelectAsk::FROM_KEY,
            args.get(SelectAsk::FROM_KEY)
                .is_some_and(|value| !value.is_null()),
        ),
        // Through the SAME reading the arm that honours it uses, so `from_here: false` — a filled-in
        // default asking for nothing — is not refused here and accepted there.
        (FROM_HERE_ARG, asks_for_here(args)?),
    ];
    for (key, given) in asked {
        if given {
            return Err(format!(
                "'{key}' says where a DIRECTION starts from, so it needs 'dir'. Give \
                 'dir' to step from that pane, or 'pane' alone to select a pane outright."
            ));
        }
    }
    Ok(())
}

/// How an answer NAMES the pane it is about on this surface: the number, plus the name if the pane
/// has one — the two handles a caller can pass back.
fn render_pane_handle(pane: &PaneRef) -> String {
    match &pane.info.name {
        Some(name) => format!("{} ({name:?})", pane.subject()),
        None => pane.subject(),
    }
}

/// What `select_pane` tells the agent, as a pure function of the outcome — so all four sentences are
/// pinned by unit tests, not only the ones a live daemon can be driven into.
///
/// `here` is the landed pane in this surface's own vocabulary ([`render_pane_handle`]), or [`None`]
/// when the listing that would name it could not be read or no longer holds it — a pane that exited
/// in the moment after the select. The `id` is the fallback subject for that case: a caller given an
/// id can still find the pane with `list_panes`, where a caller given nothing cannot.
///
/// `origin` is the pane the step was measured from, when the caller named one ([`select_origin`]).
/// [`None`] means it stepped from where the user was, which is where they still are when nothing
/// moved — so the two cases share a subject there and stop sharing one the moment an origin exists.
///
/// Every sentence says where the USER is now, because that is what this tool changes. The two
/// "nothing happened" outcomes get distinct sentences with distinct remedies, which is the whole
/// point of the daemon naming them: an edge means "look at the arrangement", a floating pane means
/// "that pane is in no arrangement, so ask for one by name".
///
/// **With an origin, the nothing-happened sentences must name the ORIGIN and the user's pane
/// separately**, because they are two panes: "there is nothing left of pane 3, so the user is still
/// on pane 1" is two facts, and collapsing them would report an edge of the pane the user is on
/// rather than of the pane the caller asked about.
fn render_selection(
    how: SelectHow,
    asked: Option<PaneDir>,
    origin: Option<&str>,
    here: Option<&str>,
    id: u64,
    elsewhere: Option<(&str, &str)>,
) -> String {
    let subject = here.map_or_else(
        || format!("the pane with id {id} (it is no longer in the pane listing — call list_panes)"),
        str::to_owned,
    );
    // ANOTHER WINDOW's select is a different sentence, and getting this wrong would have been the
    // round's own false claim. Every wording below says where the USER is, because that is what
    // this tool changes in the ordinary case — but a window has its OWN active pane, and setting
    // one the user is not looking at moves nobody. Measured: the daemon answers the far select
    // happily with `window=`, and the user's current window does not change.
    if let Some((window, current)) = elsewhere {
        let moved = match how {
            SelectHow::Moved => format!("Window {window}'s active pane is now {subject}."),
            SelectHow::AlreadyActive => {
                format!("Window {window}'s active pane was already {subject}; nothing moved.")
            }
            SelectHow::AtEdge => format!(
                "Nothing lies that way in window {window}, so its active pane is still {subject}."
            ),
            SelectHow::Untiled => format!(
                "{subject} is FLOATING, so it is in no arrangement and has no neighbour in any \
                 direction; window {window}'s active pane is unchanged."
            ),
        };
        return format!(
            "{moved} The USER is in window {current} and did not move: this sets which pane THAT \
             window is on, not which window somebody is looking at. Call select_window \
             {{\"window\": {window:?}}} to take them there, and they will land on that pane."
        );
    }
    match (how, asked, origin) {
        (SelectHow::Moved, Some(dir), None) => format!(
            "Moved the user one pane {}: they are now on {subject}.",
            dir.wire_str()
        ),
        (SelectHow::Moved, Some(dir), Some(origin)) => format!(
            "Moved the user one pane {} of {origin}: they are now on {subject}.",
            dir.wire_str()
        ),
        (SelectHow::Moved, None, _) => {
            format!("The user is now on {subject} — the active pane of this session.")
        }
        (SelectHow::AtEdge, Some(dir), None) => format!(
            "There is nothing {} {subject}, so the user is still on it: that is the edge of the \
             window. Call pane_layout to see what lies where.",
            dir.beyond()
        ),
        (SelectHow::AtEdge, Some(dir), Some(origin)) => format!(
            "There is nothing {} {origin}: that is the edge of the window, so the user is still on \
             {subject}. Call pane_layout to see what lies where.",
            dir.beyond()
        ),
        (SelectHow::Untiled, _, None) => format!(
            "The user is on {subject}, which is FLOATING: a floating pane is in no arrangement, so \
             it has no neighbour in any direction. Name the pane you want with 'pane' instead, or \
             ask the user to dock it."
        ),
        (SelectHow::Untiled, _, Some(origin)) => format!(
            "{origin} is FLOATING: a floating pane is in no arrangement, so it has no neighbour in \
             any direction, and the user is still on {subject}. Name the pane you want with 'pane' \
             instead, or ask the user to dock it."
        ),
        (SelectHow::AlreadyActive, Some(dir), Some(origin)) => format!(
            "The user was already on {subject}, which is the pane one step {} of {origin}; nothing \
             moved.",
            dir.wire_str()
        ),
        // A daemon that answered a word its request could not produce, and a plain re-select. Both
        // are honestly "nothing moved", and neither is a reason to fail a call that succeeded.
        (SelectHow::AlreadyActive | SelectHow::AtEdge, _, _) => {
            format!("The user was already on {subject}; nothing moved.")
        }
    }
}

fn tool_send_keys(args: &Value) -> Result<String, String> {
    let pane = resolve_pane_ref(args)?;
    let id = pane.id();
    let keys: Vec<String> = match args.get("keys") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|k| k.as_str().map(str::to_owned))
            .collect(),
        _ => return Err("missing required array argument 'keys' (W3C key names)".to_owned()),
    };
    if keys.is_empty() {
        return Err("'keys' must contain at least one key name".to_owned());
    }
    let flag = |name: &str| args.get(name).and_then(Value::as_bool).unwrap_or(false);
    let (ctrl, alt, shift, sup) = (flag("ctrl"), flag("alt"), flag("shift"), flag("super"));
    for key in &keys {
        host_call(
            "scene/invoke",
            with_args(
                pane_params(&pane, pane_input_path(id, KEY_ACTION)),
                json!({ "key": key, "ctrl": ctrl, "alt": alt, "shift": shift, "super": sup }),
            ),
        )?;
    }
    Ok(format!("Sent {} key(s) to {}.", keys.len(), pane.subject()))
}

/// `agent_state`: what the agent in each pane is doing, or in one named pane (H3 slice 5).
///
/// The whole-terminal form is the one an agent actually asks — "which pane needs a human" is a
/// question about the SET — so the `pane` argument is optional, unlike every read tool here. Naming a
/// pane that does not exist is still an ERROR rather than an empty answer, which is `find --pane`'s
/// rule and the CLI's: a caller who named a pane asked about that pane.
///
/// A pane with no verdict is reported EXPLICITLY rather than omitted. D3 makes that mandatory — "this
/// is not an agent" and "this agent is waiting" are opposite instructions — and an omission would
/// leave a reader to infer which by counting lines.
fn tool_agent_state(args: &Value) -> Result<String, String> {
    // Through the ONE resolver, so "is the agent in `buildout` done?" is answerable about a pane
    // one window over — which is where a sibling agent most often is, since an agent's work pane
    // and a person's reading pane are the reason a session has more than one window at all.
    let selected: Vec<PaneRef> = match resolve_optional_pane_ref(args)? {
        Some(pane) => vec![pane],
        None => {
            let panes = query_panes()?;
            if panes.is_empty() {
                return Ok("This sprag terminal has no panes.".to_owned());
            }
            panes
                .into_iter()
                .enumerate()
                .map(|(index, info)| PaneRef {
                    number: Some(numbered(index)),
                    window: None,
                    info,
                })
                .collect()
        }
    };
    let mut out = String::new();
    for pane in selected {
        match &pane.info.agent {
            Some(agent) => out.push_str(&format!(
                "  {}: id={} {}\n",
                pane.subject(),
                pane.id(),
                agent_line(agent)
            )),
            None => out.push_str(&format!(
                "  {}: id={} no agent (no manifest claims this pane — not the same as idle)\n",
                pane.subject(),
                pane.id()
            )),
        }
    }
    Ok(out)
}

/// Where this server has read the change journal up to. `None` until something anchors it, and
/// the first `wait_for_change` anchors it at the PRESENT: replaying a daemon's whole history to a
/// caller asking "what happens next" would bury the answer under a backlog it did not ask for.
static CURSOR: Mutex<Option<u64>> = Mutex::new(None);

/// Anchor the change cursor HERE, if nothing has anchored it yet.
///
/// # ⚠⚠ The race this closes, which a level would not have had
///
/// `orchestrate` returns the instant the run is submitted, and the run may finish before the agent
/// gets its next turn. If `wait_for_change` is that agent's FIRST call, its cursor starts after the
/// `run_finished` record — so it parks on an event that has already fired and waits out its whole
/// timeout for a run that ended before it looked.
///
/// So the tool that starts an asynchronous thing anchors the wait for it. The replay is bounded by
/// construction — it can only reach back to the moment the caller's own run began, which is exactly
/// the history that caller asked for.
///
/// ⚠ It NEVER moves a cursor that already exists: an agent mid-conversation has records it has not
/// read yet, and re-anchoring would drop them.
fn anchor_change_cursor() {
    let mut cursor = CURSOR.lock().unwrap_or_else(PoisonError::into_inner);
    if cursor.is_none()
        && let Ok(answer) = host_call("scene/revision", json!({}))
        && let Some(revision) = answer["revision"].as_u64()
    {
        *cursor = Some(revision);
    }
}

/// `wait_for_change`: block until this terminal's shape or an agent's verdict moves, then say what.
///
/// The tool that closes the gap between "an agent can look at other agents" and "an agent can
/// orchestrate them". Everything else here is a READ a caller has to decide when to perform, so
/// coordinating on another pane meant a poll loop — and a poll loop is a sleep chosen by whoever
/// wrote it, wrong in both directions at once.
///
/// ## ONE call, and the pair it replaces did not work
///
/// It used to park on `scene/waitFor {since}` and then read `events.<since>`. That pair is released
/// by **pane OUTPUT**, which advances the scene revision and records nothing — so against a pane
/// running a build it returned instantly with an empty batch, forever: measured on a real daemon at
/// **22 431 returns a second** (build-rate pane, every answer empty) against **zero** for a quiet one
/// — reproducible since R320 by `sprag-latency`'s poll-pair row, which also prints the zero.
/// For an agent that meant *"wait until the build in pane 2 finishes"* — the use case this tool's own
/// description names — could not be expressed at all, and each useless return cost a tool result and
/// an LLM turn.
///
/// `events/waitFor {since, match}` parks on the JOURNAL instead, and the filter is applied by the
/// daemon under the lock that appends. Output is not a record, so it cannot wake this; another pane's
/// change does not either, when the caller named its own.
///
/// ## What this costs the host, stated because a register entry claimed otherwise
///
/// R291 recorded that this tool made "three host calls, all on ONE connection, which is that
/// function's own stated rule". **That is no longer true and the rule is not restored here**, so it is
/// written down rather than quietly broken:
///
/// * `scene/revision` — FIRST CALL ONLY, on its own connection, to start the cursor at the present.
/// * a `panes` read — only when the caller passed `pane`, on its own connection, to turn this
///   surface's 1-based NUMBER into the host id the journal names. It happens BEFORE the park, so the
///   "no second connect in the middle of one question" rule is untouched: there is no question open
///   yet, and a wrong number is refused here rather than parked forever.
/// * the park itself, and — when a pane subject comes back — a `panes` read on that same connection.
///
/// The park and the read that follows it are still one connection, which was the rule's actual
/// subject. What went away is the second call that used to fetch the batch: the wait's reply carries
/// it.
///
/// ## The cursor is PROCESS state, and it has to be
///
/// Every other tool here reconnects per call and holds nothing. This one cannot: "what changed" is
/// meaningless without "since when", and an agent that had to pass a revision number back would be
/// keeping a bookkeeping detail this server already knows. So the cursor lives here, advanced by
/// each answer — which is also what makes the documented "returns immediately if something has
/// already changed since the last call" true rather than aspirational.
///
/// ## A timeout is an ANSWER, not an error
///
/// The caller asked to be told what changed; "nothing did, within the time you gave me" is a
/// truthful answer to that, and reporting it as a failure would make an agent treat a quiet
/// terminal as a broken one.
fn tool_wait_for_change(args: &Value) -> Result<String, String> {
    let timeout = match args.get("timeout_seconds") {
        None => Duration::from_secs(60),
        Some(value) => {
            let seconds = value
                .as_u64()
                .ok_or("timeout_seconds must be a whole number of seconds")?;
            if !(1..=600).contains(&seconds) {
                return Err("timeout_seconds must be between 1 and 600".to_owned());
            }
            Duration::from_secs(seconds)
        }
    };
    // Resolved BEFORE the cursor lock is taken, because it needs a host read of its own: a pane
    // NUMBER is this surface's vocabulary and the daemon's journal speaks ids.
    let filter = wait_filter(args)?;

    let mut cursor = CURSOR.lock().unwrap_or_else(PoisonError::into_inner);
    let since = match *cursor {
        Some(since) => since,
        // ⚠ THE ANSWER WITHOUT THE KEY, and the sentence has to say what that MEANS. Measured at
        // R335 against `sprag_peer::OldDaemon`: the read SUCCEEDS and carries no `revision`, so the
        // `?` above never fires and this arm is the only thing an agent hears. It used to say *"the
        // host did not report a scene revision"* — a fact with no cause and no remedy, which is
        // debt item 9's class arriving through the one path the skew ratchet did not walk.
        None => host_call("scene/revision", json!({}))?["revision"]
            .as_u64()
            .ok_or(
                "this terminal answered a scene-revision read without a revision, so there is no \
                 point in its history to wait FROM. A daemon older than this tool answers exactly \
                 that, because it keeps no event journal at all — ask the user to restart the \
                 terminal, and poll with read_pane until they do.",
            )?,
    };

    let sock = host_sock().ok_or_else(|| {
        "not inside a sprag terminal (no SPRAG_HOST_RPC_SOCK in this process or any \
         ancestor); these pane tools do not apply to this session"
            .to_owned()
    })?;
    let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)
        .map_err(|e| format!("cannot reach the sprag host at {}: {e}", sock.display()))?;

    // The caller's timeout IS the read deadline, and it is the ONLY deadline: the daemon carries
    // none, so closing this connection is what releases the park (`sprag_host::notify`).
    conn.set_read_deadline(Some(timeout))
        .map_err(|e| format!("cannot set the wait timeout: {e}"))?;
    let mut params = json!({ SINCE_PARAM: since });
    if let Some(filter) = filter {
        // Through the host's own const, not the literal `"match"`. The literal was here first, which
        // in a round whose whole thesis is that a wire word gets spelled once was this round's own
        // defect — and the CLI beside it was already using the const.
        params[EventFilter::WIRE_KEY] = filter;
    }
    let batch = match conn.try_call(EVENTS_WAIT_METHOD, params) {
        Ok(batch) => batch,
        // A filter this daemon cannot honour is the CALLER's mistake, and the daemon already wrote
        // the sentence for it — naming the offending word and the whole vocabulary it does report. It
        // reaches the agent as that sentence rather than behind `host rpc error:`, which is a
        // transport's phrase for a fault nobody could anticipate: an agent that cannot tell a typo
        // from a broken daemon retries the typo.
        //
        // Matched on the fault's CODE, never on its rendered line — the rule
        // `sprag::unknown_slot` records, because a substring test against a rendering is a test
        // against a presentation decision.
        Err(CallError::Fault(fault)) if fault.code == INVALID_PARAMS => {
            return Err(fault
                .data
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or(&fault.message)
                .to_owned());
        }
        // A connection that trips its deadline is finished, which is fine: nothing happened, and
        // the cursor has not moved, so the next call parks from the same place.
        //
        // BOTH kinds, because a socket read timeout is not one error. `std` says so of
        // `set_read_timeout` — "WouldBlock or TimedOut" — and Linux is the `WouldBlock` half
        // (EAGAIN), which is what the live gate caught: matching only `TimedOut` turned every quiet
        // wait into a tool failure reading `Resource temporarily unavailable`.
        Err(CallError::Transport(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            return Ok(format!(
                "Nothing changed in {} seconds. The terminal is quiet; call again to keep waiting.",
                timeout.as_secs()
            ));
        }
        // A filter this daemon cannot honour is the CALLER's mistake, and the daemon already wrote
        // the sentence for it — naming the offending word and the whole vocabulary it does report. It
        // reaches the agent as that sentence rather than behind `host rpc error:`, which is a
        // transport's phrase for a fault nobody could anticipate. An agent that cannot tell a typo
        // from a broken daemon retries the typo.
        Err(other) => return Err(std::io::Error::from(other).to_string()),
    };
    conn.set_read_deadline(None)
        .map_err(|e| format!("cannot clear the wait timeout: {e}"))?;

    *cursor = Some(batch["next"].as_u64().unwrap_or(since));
    drop(cursor);

    let mut out = String::new();
    if batch["lost"].as_bool().unwrap_or(false) {
        out.push_str(
            "Some changes were dropped before this call could read them, so this list is \
             incomplete. Re-read the terminal with list_panes.\n",
        );
    }
    let events = batch["events"].as_array().map_or(&[][..], Vec::as_slice);
    if events.is_empty() {
        // Reachable only through `lost`: the wait does not return without a matching record
        // otherwise. It used to be the ANSWER for a chatty terminal, which is the defect this
        // rewrite removed.
        out.push_str("Changes were dropped and none of the survivors matched what you asked for.");
        return Ok(out);
    }
    // The wire names a pane by the HOST's id; every tool on this surface addresses one by its
    // 1-based NUMBER. So the ids are joined against the pane list before they are printed — over the
    // connection the wait was parked on, because the list must be read AFTER the change rather than
    // before it (a `pane_closed` names a pane the earlier list still had).
    //
    // The reason this comment used to give — "for the reason the park and the batch already share one
    // connection" — named a pairing that no longer exists: the wait's reply CARRIES the batch, so
    // there is no second call to share anything with. Corrected rather than left standing, on this
    // project's own rule that the owed item is often the comment.
    //
    // **The comment below used to claim this and the code did not do it**, which made the answer
    // name a pane in a vocabulary no other tool here accepts: an agent told `pane id=0` cannot pass
    // `0` to `pane_processes`, whose pane numbers start at 1. Found by reading what the tool
    // actually printed.
    //
    // Read only when a pane subject is present: a window or session change pays nothing for it.
    //
    // BOTH listings, for [`process_row_subject`]'s reason: the journal is the SESSION's, so an
    // event about a pane in another window would otherwise be reported as gone when it is simply
    // elsewhere — the same false sentence, in the tool an agent reads to find out what changed.
    let (here, session) = if events.iter().any(|event| event["pane"].is_u64()) {
        (query_panes()?, query_session_panes()?)
    } else {
        (Vec::new(), Vec::new())
    };

    out.push_str(&render_events(events, &here, &session));
    Ok(out)
}

/// Build [`tool_wait_for_change`]'s `match` parameter from the tool's own arguments, or `None` for a
/// caller that named nothing and wants every change.
///
/// ## The translation this exists for
///
/// Every tool on this surface addresses a pane by its 1-based NUMBER; the daemon's journal names one
/// by its id. So a `pane` argument costs a pane-list read before the wait can be issued — the same
/// join [`tool_wait_for_change`] already does on the way OUT, done on the way in. That read is what
/// also makes a wrong number a refusal ("no pane 7; this terminal has 3") instead of a wait that can
/// never return.
///
/// ## Why `kinds` is a LIST and pairs with `pane` as a product
///
/// `{pane: 2, kinds: [pane_job_changed, pane_closed]}` is *wake me when pane 2's job changes or pane
/// 2 disappears* — the two ways the thing an agent waits for can end. One clause per kind, each
/// carrying the pane, which is the daemon's any-of form. A caller naming only `kinds` gets those kinds
/// for any subject; one naming only `pane` gets everything about that pane.
///
/// A kind is passed THROUGH rather than validated here: the daemon owns the vocabulary and refuses an
/// unknown word with the whole list of what it does report, so validating here would be a second
/// enumeration of the exact kind this round removed from the host.
fn wait_filter(args: &Value) -> Result<Option<Value>, String> {
    let kinds: Vec<String> = match args.get("kinds") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|kind| {
                kind.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "each entry of kinds must be a string".to_owned())
            })
            .collect::<Result<Vec<String>, String>>()?,
        Some(_) => return Err("kinds must be a list of change names".to_owned()),
    };
    // Through the same resolver every other tool's `pane` argument uses, so a wrong number is
    // refused in the one sentence this surface already says for it.
    let pane = match args.get("pane") {
        None | Some(Value::Null) => None,
        Some(_) => Some(resolve_pane_ref(args)?.id()),
    };
    Ok(EventFilter::narrowing_wire(pane, &kinds))
}

/// One line per change: `  <type>: <subject>`, with a pane named in BOTH vocabularies.
///
/// A pure function of the two reads, for the reason its neighbours are
/// ([`pane_processes`](tool_pane_processes) and the layout drawing both render this way): the
/// interesting case is the RESIDUAL between them — a pane in the batch that is no longer in the
/// list — and that is not reachable through a live daemon on demand. Inline in the caller it was
/// testable by nothing at all.
fn render_events(events: &[Value], here: &[PaneInfo], session: &[(String, PaneInfo)]) -> String {
    let mut out = String::new();
    for event in events {
        let kind = event["type"].as_str().unwrap_or("?");
        match (
            event[sprag_host::events::Subject::PANE_KEY].as_u64(),
            event[sprag_host::events::Subject::WINDOW_KEY].as_str(),
            event[sprag_host::events::Subject::SESSION_KEY].as_str(),
            event[sprag_host::events::Subject::RUN_KEY].as_u64(),
        ) {
            (Some(id), _, _, _) => {
                // Both integers travel: the number is what this surface's tools take, and the id is
                // what `sprag panes`, the daemon's logs and the user's own CLI call the same pane,
                // so an agent reporting to a human and a human checking the agent hold one picture.
                out.push_str(&format!(
                    "  {kind}: {}\n",
                    // EMPTY registry on purpose: an event stream is scoped to the
                    // caller's own session, so a pane it names and this listing cannot find really
                    // is one that ended — which is the case the last arm is for.
                    process_row_subject(id, here, session, &[])
                ));
            }
            (_, Some(name), _, _) => out.push_str(&format!("  {kind}: window {name}\n")),
            (_, _, Some(name), _) => out.push_str(&format!("  {kind}: session {name}\n")),
            // ⚠ THE FOURTH SUBJECT, and it was NOT here when `run_finished` was added — the event
            // reached the agent with its id dropped, so a caller with two loops in flight was told
            // one of them had finished and not which. A hand-written match over subject keys is the
            // list a new subject is left out of; `every_subject_an_event_names_is_rendered_with_it`
            // is what makes the next one fail instead.
            (_, _, _, Some(id)) => out.push_str(&format!("  {kind}: run {id}\n")),
            _ => out.push_str(&format!("  {kind}\n")),
        }
    }
    out
}

/// `agent_explain`: which RULE produced a pane's state, and what to edit when it is wrong.
///
/// H3's D7 is the whole reason this can exist as a READ: the rule's identity travels in every verdict
/// and on the wire, so this reports the value the detector produced rather than evaluating anything a
/// second time. A second evaluation is a second answer, and two answers about one pane is the defect
/// D2 avoids one layer down.
///
/// The remedy is named because a rule id with no instruction is only half an explanation: the id is
/// what an `[[agent]]` block in `config.toml` addresses — replacing it in place, or `disable`-ing it —
/// which is what makes a mis-detected pane fixable by the user who found it rather than by a release.
///
/// And a remedy that names a file is worth nothing if the file is already unreadable, which is why
/// every answer here is prefixed by [`manifest_caveat`] when there is one. The `sprag agent` verb
/// does the same thing for a person; this is the same fact reaching the reader that acts on it.
fn tool_agent_explain(args: &Value) -> Result<String, String> {
    // Through the ONE resolver, and the agent's verdict rides on the pane it resolved — so this
    // answers about a sibling agent wherever it is working, which is the question the tool exists
    // for. Its twin `agent_state` reaches the same way and for the same reason.
    let pane = resolve_pane_ref(args)?;
    let subject = pane.subject();
    // In front of EVERY branch below, and most of all the one that says no manifest claims this
    // pane: that sentence is also what an unparsed claim looks like from here, and sending a reader
    // off to write an `[[agent]]` block they have already written is the trap this closes.
    let mut out = manifest_caveat().unwrap_or_default();
    let Some(agent) = &pane.info.agent else {
        out.push_str(&format!(
            "{subject} (id={}) has no agent state: no agent manifest claims this pane, so no \
             rule was even consulted for it. That is what an ordinary shell looks like. If this pane \
             IS running an agent sprag does not know, add an `[[agent]]` block to sprag's config.toml \
             with a fingerprint that matches its screen or title.\n",
            pane.id()
        ));
        return Ok(out);
    };
    out.push_str(&format!("{subject} (id={}) is {}", pane.id(), agent.state));
    match &agent.name {
        Some(name) => out.push_str(&format!(", detected as `{name}`")),
        None => out.push_str(
            ", and no manifest is currently identified for it (a dialog can cover the very lines an \
             agent's fingerprint is read from, so the state is published without the name)",
        ),
    }
    match &agent.rule {
        Some(rule) => out.push_str(&format!(
            ". The rule that fired is `{rule}`. If that verdict is wrong, redefine or `disable` the \
             rule with that id in an `[[agent]]` block in sprag's config.toml — a corrected rule \
             keeps its position, and the daemon picks the edit up on its own.\n"
        )),
        None => out.push_str(
            ". No rule id came with the verdict, which is a pre-H3 daemon rather than an \
             unexplainable state.\n",
        ),
    }
    out.push_str(&format!(
        "The state has changed {} time(s) since this pane was first seen (seq={}), so a repeat read \
         showing the same seq is the same verdict rather than a new one.\n",
        agent.seq, agent.seq
    ));
    Ok(out)
}

// ---- Pane resolution + host bridge ---------------------------------------------

/// How a tool's caller named the pane it means.
///
/// **Two spellings, discriminated by JSON's own types**, and that is the whole reason a pane has a
/// name at all. A NUMBER and a host ID are both integers, so one argument could never carry both
/// without a mode flag on the most-used argument of this surface. A name is a string, so it can.
#[derive(Debug, PartialEq, Eq)]
enum PaneTarget {
    /// The 1-based position in the listing — convenient and POSITIONAL: closing an earlier pane
    /// shifts it, so a remembered number can come to mean a different pane.
    Number(usize),
    /// The pane's own name, which never moves.
    Name(String),
}

/// The pane a tool's arguments name, under the `key` that names it — `pane` for almost every tool,
/// and a swap's `with` or a directional select's `from` for the two that take a second one. Those
/// are pane handles in every respect except their key, so they parse through the same grammar and
/// say the same things about a bad one.
///
/// **Reached only through [`resolve_pane_ref_at`].** A tool that parsed a target and looked it up
/// itself would be looking in one window, which is how eleven of eighteen came to be window-local.
fn pane_target_at(args: &Value, key: &str) -> Result<PaneTarget, String> {
    match args.get(key) {
        Some(Value::Number(n)) => {
            let n = n
                .as_u64()
                .ok_or_else(|| format!("the '{key}' number must be a positive whole number"))?;
            usize::try_from(n)
                .map(PaneTarget::Number)
                .map_err(|_| format!("the '{key}' number is out of range"))
        }
        // A name is trimmed here as well as in the daemon, so `pane: " build "` resolves rather
        // than reporting that no pane is called that. This is the RESOLVER, not a second parse: it
        // applies no rule the daemon does not, and a name that breaks one simply matches nothing.
        Some(Value::String(name)) => Ok(PaneTarget::Name(name.trim().to_owned())),
        _ => Err(format!(
            "missing required argument '{key}': a NUMBER (1-based, see list_panes) or a pane's NAME"
        )),
    }
}

/// Find the pane a caller named in ONE reading of ONE window's listing, answering its position and
/// its row.
///
/// **Private to [`resolve_pane_ref`] by discipline, and by the ratchet that enforces it.** A tool
/// that called this against a listing of its own would be window-local — which is exactly how
/// eleven of the eighteen pane-addressed tools came to refuse a pane one window over, measured at
/// `e7be5eb`. `the_whole_roster_reaches_a_pane_one_window_over` drives every tool the roster
/// declares a `pane` argument for and fails the moment one of them stops going through the
/// resolver.
fn resolve_in<'a>(
    panes: &'a [PaneInfo],
    target: &PaneTarget,
) -> Result<(usize, &'a PaneInfo), String> {
    match target {
        PaneTarget::Number(number) => panes
            .iter()
            .enumerate()
            .find(|(index, _)| numbered(*index) == *number)
            .ok_or_else(|| {
                format!(
                    "no pane {number}; this terminal has {} pane(s). Call list_panes.",
                    panes.len()
                )
            }),
        PaneTarget::Name(name) => pane_by_name(panes, name),
    }
}

/// The 1-based NUMBER of the row at `index` of a listing — the one arithmetic that turns a position
/// into what a caller types, spelled once so the surface cannot come to disagree with itself about
/// whether the first pane is 0 or 1.
const fn numbered(index: usize) -> usize {
    index + 1
}

/// A pane this surface has resolved: WHICH pane, WHICH WINDOW, and everything the listing said
/// about it — all from the one reading that resolved it.
///
/// The window is [`None`] for a pane of the caller's own window, which is every pane a NUMBER can
/// name and most panes a name does. It is `Some` only for a pane a NAME reached one window over —
/// and it is what every request below must carry, because both a scene READ and the mux actions
/// that resolve against the scope are answered for ONE window ([`sprag_rpc::WINDOW_PARAM`]).
///
/// # It carries the pane's INFO, and that is what keeps the read whole
///
/// R311 needed the pane's NUMBER in a write's answer and wrote a second resolver beside this one —
/// a second query behind a single argument parse, which is verbatim the torn read this module's
/// own docs warn about. The lesson generalises: any fact a tool wants about the pane it just
/// resolved must come from the reading that resolved it, or the tool names one pane and answers
/// about another. So the whole row rides along and there is nothing left to go back for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneRef {
    /// Its 1-based number in `list_panes`, or [`None`] for a pane in ANOTHER window.
    ///
    /// `None` rather than the position within that other window, deliberately: a number on this
    /// surface means "the Nth pane of YOUR window", and handing back a number that means something
    /// else is the positional confusion the whole name grammar exists to remove.
    number: Option<usize>,
    /// The window it lives in, or [`None`] for the caller's own.
    window: Option<String>,
    /// The listing row, read in the SAME query that resolved this pane.
    info: PaneInfo,
}

impl PaneRef {
    /// The pane's host id — what the wire takes.
    fn id(&self) -> u64 {
        self.info.id
    }

    /// How an ANSWER names this pane — its number when the caller could have used one, else its
    /// id and the window it is in, which is the only honest handle for a pane one window over.
    fn subject(&self) -> String {
        match (&self.number, &self.window) {
            (Some(number), _) => format!("pane {number}"),
            (None, Some(window)) => format!("pane id {} (window {window})", self.id()),
            (None, None) => format!("pane id {}", self.id()),
        }
    }
}

/// Resolve a tool's `pane` argument to a pane and the window it is in.
///
/// # A NUMBER is window-local and a NAME is not, and that asymmetry is the CONTRACT
///
/// A number is defined by `list_panes` — "the Nth pane of this window" — so widening it across the
/// session would silently change what every existing `pane: 2` means, which is the positional
/// hazard this whole surface exists to keep an agent away from.
///
/// A NAME was always promised wider: [`sprag_terminal::PaneName`] is unique REGISTRY-wide and
/// exists precisely so an agent holds a handle that does not move. Until R311 it was resolved only
/// against the caller's own window, so `read_pane {pane: "buildout"}` one window over answered
/// *"no pane is called \"buildout\"; no pane in this terminal has a name yet"* — **both halves
/// false**, measured against a real daemon. Meanwhile `rename_pane` and `swap_pane` crossed a
/// window freely, because a write is a mux action and a read is a scene path.
fn resolve_pane_ref(args: &Value) -> Result<PaneRef, String> {
    resolve_pane_ref_at(args, SelectAsk::PANE_KEY)
}

/// [`resolve_pane_ref`] for an argument spelled something other than `pane` — a swap's partner and
/// a directional select's origin are pane handles in every respect except their key, so they
/// resolve through the same grammar and reach as far.
fn resolve_pane_ref_at(args: &Value, key: &str) -> Result<PaneRef, String> {
    let target = pane_target_at(args, key)?;
    let here = query_panes()?;
    match resolve_in(&here, &target) {
        Ok((index, pane)) => Ok(PaneRef {
            number: Some(numbered(index)),
            window: None,
            info: pane.clone(),
        }),
        // Only a NAME looks further: a number that missed named no pane of the window it is defined
        // against, and looking elsewhere would answer about a pane the caller did not ask for.
        Err(near) => match &target {
            PaneTarget::Number(_) => Err(near),
            PaneTarget::Name(name) => match query_session_panes()? {
                elsewhere if elsewhere.is_empty() => Err(near),
                elsewhere => {
                    let (window, pane) = pane_by_name_in_session(&elsewhere, name)?;
                    Ok(PaneRef {
                        number: None,
                        window: Some(window),
                        info: pane.clone(),
                    })
                }
            },
        },
    }
}

/// [`resolve_pane_ref`] for an OPTIONAL `pane` argument — absent (or `null`) answers [`None`],
/// which every tool here reads as "the pane I am in" or "all of them" depending on what it does.
///
/// Spelled once because the absent-means-default check is where a tool most easily forgets to go
/// through the resolver at all: it already has a branch, and hand-rolling the present arm inside
/// it is how five tools came to be window-local.
fn resolve_optional_pane_ref(args: &Value) -> Result<Option<PaneRef>, String> {
    match args.get(SelectAsk::PANE_KEY) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => resolve_pane_ref(args).map(Some),
    }
}

/// A pane-addressed `params` object with an invoke's `args` filled in — [`pane_params`] for the
/// write paths, so a window narrowing rides on those too rather than only on the reads.
fn with_args(params: Value, args: Value) -> Value {
    let mut params = params;
    if let Value::Object(map) = &mut params {
        map.insert("args".to_owned(), args);
    }
    params
}

/// The `params` a pane-addressed query or invoke sends, carrying the pane's window when it has one.
///
/// The ONE place a request learns which window to look in, so a tool added later cannot forget it
/// and quietly become window-local again.
///
/// # Every pane-addressed request, not only the reads
///
/// Measured against a live daemon at `e7be5eb`: a scene READ of a pane's own external answers
/// `NoExternalAtPath` without a window and the pane's text with one, and the mux actions divide
/// into two kinds — `rename_pane` / `swap_pane` / `zoom_pane` resolve REGISTRY-wide and
/// `select_pane` / `close` / `split` resolve against the SCOPE's window, refusing bare and
/// succeeding with `window=`. Sending the window on every one of them is therefore correct for both
/// kinds: the registry-wide actions do not consult it, and the scope-local ones need it. A client
/// that tried to remember which is which would be keeping a second copy of the daemon's own rule.
fn pane_params(pane: &PaneRef, path: String) -> Value {
    windowed_params(path, pane.window.as_deref())
}

/// [`pane_params`] for a request addressed at a WINDOW rather than at a pane in it.
fn windowed_params(path: String, window: Option<&str>) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("path".to_owned(), Value::String(path));
    if let Some(window) = window {
        map.insert(
            sprag_rpc::WINDOW_PARAM.to_owned(),
            Value::String(window.to_owned()),
        );
    }
    Value::Object(map)
}

/// One window's pane listing.
///
/// The rows carry no numbers and cannot: a number is `list_panes`'s row index and `list_panes`
/// answers about the CALLER's window, so numbering another window's panes would hand back numbers
/// that name different panes. See [`PaneInfo`].
fn query_window_panes(window: &str) -> Result<Vec<PaneInfo>, String> {
    let value = host_call(
        "scene/query",
        windowed_params(mux_action_path(PANES_SLOT), Some(window)),
    )?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .map(parse_pane_info)
        .collect())
}

/// Every pane of the caller's SESSION, paired with the window it is in — one query per window.
///
/// Read window by window rather than from one session-wide slot because the daemon has no such
/// slot and should not grow one for this: the `panes` slot is a WINDOW's pane list by construction
/// (it is what a display client projects), and R311 gave a request the ability to name which
/// window rather than inventing a second listing that could disagree with the first.
fn query_session_panes() -> Result<Vec<(String, PaneInfo)>, String> {
    let mut out = Vec::new();
    for window in query_window_names()? {
        for pane in query_window_panes(&window)? {
            out.push((window.clone(), pane));
        }
    }
    Ok(out)
}

/// The caller's session's windows, in the order the session arranges them (R310) — each one's
/// name, whether the session is ON it, and WHO ASKED for it.
///
/// Through the SSOT type, never by reading the keys by hand: a second reader of the served shape is
/// a second thing that can come to disagree with the daemon about what a field means, which is the
/// rule `PaneProcessesWire` and `LayoutSnapshot` already follow here.
fn query_windows() -> Result<Vec<WindowInfo>, String> {
    let value = host_call(
        "scene/query",
        json!({ "path": mux_action_path(WINDOWS_SLOT) }),
    )?;
    serde_json::from_value(value)
        .map_err(|error| format!("the host's window list did not parse: {error}"))
}

/// Just the NAMES, for the readers that only walk them.
fn query_window_names() -> Result<Vec<String>, String> {
    Ok(query_windows()?
        .into_iter()
        .map(|window| window.name)
        .collect())
}

/// Find the pane called `name` anywhere in the SESSION, refusing to guess between two bearers.
///
/// [`pane_by_name`]'s rule one scope wider, and the refusal is what R311 came to fix: it names the
/// session's named panes and the windows they are in, where the old sentence said *"no pane in this
/// terminal has a name yet"* about a terminal that had one.
fn pane_by_name_in_session<'a>(
    panes: &'a [(String, PaneInfo)],
    name: &str,
) -> Result<(String, &'a PaneInfo), String> {
    let bearers = |wanted: Option<&str>| -> Vec<NamedPane> {
        panes
            .iter()
            .filter_map(|(window, pane)| {
                let held = pane.name.as_deref()?;
                (wanted.is_none_or(|wanted| wanted == held))
                    .then(|| NamedPane::new(held, window.clone()))
            })
            .collect()
    };
    let mut matching = panes
        .iter()
        .filter(|(_, pane)| pane.name.as_deref() == Some(name))
        .fuse();
    let first = matching
        .next()
        .ok_or_else(|| unknown_pane_name_with(name, &bearers(None), PaneListing::ListWindows))?;
    if matching.next().is_some() {
        return Err(ambiguous_pane_name(name, &bearers(Some(name))));
    }
    Ok((first.0.clone(), &first.1))
}

/// Find the pane called `name` in `panes`, refusing to guess when more than one answers to it.
///
/// The daemon holds names unique across itself, so a second bearer cannot arise from a correct
/// sequence of requests — but the uniqueness check and the write are not one atomic step there, so
/// this refuses rather than taking the first. Silently resolving an ambiguous name would rebuild
/// the very failure a name exists to remove: a plausible answer against the wrong pane.
fn pane_by_name<'a>(panes: &'a [PaneInfo], name: &str) -> Result<(usize, &'a PaneInfo), String> {
    let mut matching = panes
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name.as_deref() == Some(name))
        .fuse();
    // Neither sentence below normally reaches a caller: [`resolve_pane_ref`] treats a miss HERE as
    // "look in the rest of the session" and the session-wide sentence is what a caller reads. They
    // are written honestly anyway, because they are what a near-only resolution would say and a
    // sentence nobody checks is how the false one R311 removed survived.
    let first = matching
        .next()
        .ok_or_else(|| format!("no pane is called {name:?} in this window."))?;
    if matching.next().is_some() {
        return Err(format!(
            "more than one pane in this window is called {name:?}, so it does not name one pane."
        ));
    }
    Ok(first)
}

/// Query the host's live pane list, numbered 1-based in host order.
fn query_panes() -> Result<Vec<PaneInfo>, String> {
    let value = host_call(
        "scene/query",
        windowed_params(mux_action_path(PANES_SLOT), None),
    )?;
    let array = value
        .as_array()
        .ok_or("the host pane list was not an array")?;
    Ok(array.iter().map(parse_pane_info).collect())
}

/// Why the daemon's agent manifests are not the ones the user's `config.toml` declares, as a line to
/// put in front of an explanation — or nothing when they are, which is the ordinary case.
///
/// A read failure answers `None` rather than propagating: this is a CAVEAT on another tool's answer,
/// and an old daemon that does not serve the slot must not turn `agent_explain` into an error. The
/// tool it qualifies has already made its own call and would report a dead host itself.
fn manifest_caveat() -> Option<String> {
    let value = host_call(
        "scene/query",
        json!({ "path": mux_action_path(AGENT_MANIFESTS_SLOT) }),
    )
    .ok()?;
    let error = value.get("error").and_then(Value::as_str)?;
    Some(manifest_caveat_line(error))
}

/// The caveat's WORDING, split from the call that fetches it so it is testable without a live host —
/// [`parse_pane_info`]'s reason, applied to the other direction of the same boundary.
///
/// It says three things a reader needs and would not otherwise reach: that the file is broken, that
/// detection did NOT fall back to nothing (the daemon kept the last usable list, so the verdicts
/// below are real answers from a stale rule set), and that an unparsed claim is indistinguishable
/// from an absent one. The last is the trap: without it, `no agent manifest claims this pane` sends
/// a reader to write an `[[agent]]` block they have already written.
fn manifest_caveat_line(error: &str) -> String {
    format!(
        "NOTE before reading this: sprag's config.toml does not currently declare usable agent \
         manifests ({error}). The daemon is detecting with the last list that worked, so a verdict \
         below may be answering to a rule the file no longer contains — and a pane the file was \
         meant to claim can appear as if no manifest claims it. Fixing that file is the first \
         move.\n"
    )
}

/// Parse one panes-slot entry into a [`PaneInfo`].
/// Every field is ADDITIVE on the wire (present only when its signal fired), so a missing key maps
/// to the resting default — split out as a pure function so the parse is testable without a live
/// host (mirrors [`parse_image_info`]).
///
/// It takes no position, and that is the point: see [`PaneInfo`]'s note on why a row carries no
/// number. The caller that HOLDS a listing forms the numbers with [`numbered`].
fn parse_pane_info(pane: &Value) -> PaneInfo {
    PaneInfo {
        id: pane.get("id").and_then(Value::as_u64).unwrap_or(0),
        name: pane.get("name").and_then(Value::as_str).map(str::to_owned),
        title: pane
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        command: pane
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cols: pane.get("cols").and_then(Value::as_u64).unwrap_or(0),
        rows: pane.get("rows").and_then(Value::as_u64).unwrap_or(0),
        notification: pane.get("notification").map(notification_line),
        bell: pane.get("bell_seq").and_then(Value::as_u64).unwrap_or(0),
        shell: pane.get("shell").and_then(Value::as_str).map(str::to_owned),
        exit_status: pane.get("exit_status").and_then(Value::as_i64),
        mouse: pane.get("mouse").and_then(Value::as_str).map(str::to_owned),
        focus_tracking: pane
            .get("focus_tracking")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        images: pane
            .get("images")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(parse_image_info).collect())
            .unwrap_or_default(),
        active: pane.get("active").and_then(Value::as_bool).unwrap_or(false),
        agent: parse_agent_info(pane),
        opened_by: pane.get("opened_by").and_then(Value::as_u64),
    }
}

/// Format the panes slot's `notification` object (`{title, body, seq}`) into one display line —
/// `"title" — body` when both are present, just the body or title otherwise. Missing fields fall
/// away so a title-only (kitty) or body-only (OSC 9) notification reads cleanly.
fn notification_line(note: &Value) -> String {
    let title = note
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = note.get("body").and_then(Value::as_str).unwrap_or_default();
    match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title:?} — {body}"),
        (false, true) => format!("{title:?}"),
        _ => body.to_owned(),
    }
}

/// One request to the host over a fresh connection, mapping every failure to a
/// human-readable tool error (including "not inside a sprag terminal").
/// Replace a daemon REFUSAL with a sentence this tool can write, and pass anything else through.
///
/// **Decided by the fault's KIND, never by its rendering** — the discipline R292 established after
/// matching on wording had already cost a round. Three kinds, three answers:
///
/// * [`io::ErrorKind::InvalidInput`] — the daemon refused and STATED WHY (R325 /
///   `sprag_host::wire::refusal`). Its sentence is the answer, ALONE: appending this surface's
///   `instead` beside it would put a guess back next to a fact, which is the one thing this round
///   removed. `instead` is unreachable against any daemon of this build.
/// * [`io::ErrorKind::Other`] — a bare refusal, which on this wire means a daemon older than
///   PINION-PR82. Only there does this surface's own sentence get written, and only because that
///   daemon genuinely could not say which cause it was.
/// * anything else — a transport failure or a skew, kept and annotated. "The socket went away" and
///   "the daemon said no" are different things to be told.
fn refusal_sentence((raw, kind): &(String, io::ErrorKind), instead: &str) -> String {
    match *kind {
        io::ErrorKind::InvalidInput => raw.clone(),
        io::ErrorKind::Other => instead.to_owned(),
        _ => format!("{raw} — {instead}"),
    }
}

/// [`host_call`], keeping the failure's ERROR KIND so a caller can tell a REFUSAL from a transport
/// failure ([`refusal_sentence`]). The plain form drops it, because every other tool here reports
/// the daemon's own sentence unchanged.
fn host_call_kinded(method: &str, params: Value) -> Result<Value, (String, io::ErrorKind)> {
    host_call_unscoped(method, in_our_session(params))
}

fn host_call(method: &str, params: Value) -> Result<Value, String> {
    host_call_kinded(method, params).map_err(|(sentence, _)| sentence)
}

/// [`host_call_kinded`] WITHOUT the ambient scope — the one request that must not carry it, because
/// it is the request that works out what it would be.
///
/// It is also the whole of the transport: the two public forms above differ only in how much of a
/// failure they hand back, and they were two copies of this body until the scope needed stamping in
/// exactly one place. Two doors onto one act is the shape this project keeps finding defects in.
fn host_call_unscoped(method: &str, params: Value) -> Result<Value, (String, io::ErrorKind)> {
    let sock = host_sock().ok_or_else(|| {
        (
            "not inside a sprag terminal (no SPRAG_HOST_RPC_SOCK in this process or any \
             ancestor); these pane tools do not apply to this session"
                .to_owned(),
            io::ErrorKind::NotFound,
        )
    })?;
    let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT).map_err(|e| {
        let kind = e.kind();
        (
            format!("cannot reach the sprag host at {}: {e}", sock.display()),
            kind,
        )
    })?;
    // Both built BEFORE the call, which consumes the params.
    let label = sprag_rpc::request_label(method, &params);
    let path = params["path"].as_str().unwrap_or_default().to_owned();
    match conn.try_call(method, params) {
        Ok(answer) => Ok(answer),
        // TWO library sentences before this surface writes anything: the daemon does not have that
        // address at all (a skew), or it HAS it, refused, and said why (R325). An agent gets the
        // producer's own fact for the second, which is the whole of PINION-PR82's value here —
        // eight of these tools used to hand an agent a client-side list of guesses.
        Err(CallError::Fault(fault)) => Err(older_daemon(method, &path, &fault)
            .or_else(|| sprag_host::wire::refusal(&fault))
            .map_or_else(
                // Rendered exactly as `HostConn::call` would have, through the same function it
                // uses, so a fault no library sentence covers reads as it always did.
                || (format!("{label}: {fault}"), io::ErrorKind::Other),
                |stated| (stated.to_string(), stated.kind()),
            )),
        Err(CallError::Transport(error)) => {
            let kind = error.kind();
            Err((error.to_string(), kind))
        }
    }
}

/// The sentence for a daemon that has never heard of what was just asked of it — or [`None`] for a
/// fault it produced on purpose, which is the caller's own business.
///
/// # Why an agent needs this more than an operator does
///
/// A slot and an action are both additive, so `WIRE_PROTOCOL` does not rise when either is added
/// and a client that gained one meets same-numbered daemons that lack it. The CLI has told the two
/// apart since R321/R322; this surface never had. Measured against a peer that serves nothing and
/// knows no verb, **eight of eight tools** got it wrong: six printed `UnknownIntrospectPath` at an
/// AGENT, and `display_message` answered *"the message may be unacceptable (it must be one line,
/// under 200 bytes …)"* about a message that broke none of those rules.
///
/// The kind is what stops the tools' own sentences from replacing it: [`refusal_sentence`] swaps
/// its own words in only for [`io::ErrorKind::Other`], so a skew — which is
/// [`io::ErrorKind::Unsupported`] — reaches the agent WITH the tool's context rather than instead
/// of it.
fn older_daemon(method: &str, path: &str, fault: &sprag_rpc::RpcFault) -> Option<io::Error> {
    match method {
        "scene/query" => sprag_host::wire::unknown_slot(path, fault),
        "scene/invoke" => sprag_host::wire::unknown_action(path, fault),
        _ => None,
    }
}

/// Stamp the session this server's PANE is in onto a request that names none.
///
/// # What an agent was being told before this
///
/// Every request here went out unscoped, so the daemon answered about ITS default session. An agent
/// working in a pane of `work` asked `list_panes` and was listed the panes of session `0` —
/// measured, with the boot pane of a session it is not in coming back as *"1 pane(s) in this
/// window"*. It is the same defect the `sprag` CLI had, and it is worse here: a person at a shell
/// can see which session they are in, and an agent's pane is the only thing it knows about its own
/// position.
///
/// A caller that already named a session keeps it — nothing here does today, and a stamp that
/// overwrote one would be a scope this server invented.
fn in_our_session(mut params: Value) -> Value {
    if let (Some(session), Some(map)) = (our_session(), params.as_object_mut())
        && !map.contains_key(sprag_host::wire::SESSION_PARAM)
    {
        map.insert(
            sprag_host::wire::SESSION_PARAM.to_owned(),
            Value::String(session.to_owned()),
        );
    }
    params
}

/// The session holding [`own_pane`], asked once.
///
/// [`None`] when this server is not running in a pane of this daemon — a client that forwarded the
/// socket and not the id, an id left over from a daemon that has exited, or a daemon too old to
/// serve the tree. Each of those means the same thing to a caller (*nobody said which session*),
/// which is the behaviour that was already there, so none of them is worth an error.
fn our_session() -> Option<&'static str> {
    static OURS: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    OURS.get_or_init(|| {
        let pane = own_pane()?;
        let answer = host_call_unscoped(
            "scene/query",
            json!({ "path": mux_action_path(sprag_host::wire::TREE_SLOT) }),
        )
        .ok()?;
        let tree: Vec<sprag_terminal::TreeSession> = serde_json::from_value(answer).ok()?;
        // Through the daemon's own reader, shared with the `sprag` CLI, so the tool an agent reads
        // with and the command it acts with cannot disagree about which session its pane is in.
        sprag_host::wire::session_holding(&tree, sprag_terminal::PaneId(pane)).map(str::to_owned)
    })
    .as_deref()
}

/// Resolve the host socket path: this process's `SPRAG_HOST_RPC_SOCK`, else the first
/// `/proc` ancestor that carries it (the `sprag-term` host in our own process tree).
fn host_sock() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCK_ENV) {
        return Some(PathBuf::from(path));
    }
    ancestor_pids()
        .into_iter()
        .find_map(|pid| read_proc_env(pid, SOCK_ENV))
        .map(PathBuf::from)
}

/// Our ancestors' process ids, NEAREST FIRST — bounded so a broken `/proc` (or a PID cycle) can
/// never loop forever, and stopping at the first process `/proc` will not describe.
///
/// One walk with two readers ([`host_sock`] and [`own_pane`]), because the walk IS what makes this
/// server self-configuring in any pane: a second copy could come to disagree about how far up to
/// look, and the integration harness asserts this one's precedence as a SAFETY property — a
/// resolver that reached the wrong daemon would type into the author's own panes.
fn ancestor_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..64 {
        let Some(ppid) = read_ppid(pid) else { break };
        if ppid == 0 || ppid == pid {
            break;
        }
        pids.push(ppid);
        pid = ppid;
    }
    pids
}

/// The parent PID of `pid`.
///
/// ⚠ Through [`sprag_terminal::procfs`] rather than `/proc/<pid>/status`, which is what this read
/// and the one below it. That made the WHOLE ancestor walk — the thing that answers *"which daemon
/// is this agent running under?"* — silently answer nothing on any platform without `/proc`, and
/// the first macOS run of this suite is what said so. The reader lives beside the crate that
/// already owns a process's other facts, so there is one place per platform rather than one per
/// caller.
fn read_ppid(pid: u32) -> Option<u32> {
    sprag_terminal::procfs::parent(pid)
}

/// Read `key`'s value from the environment `pid` was EXEC'd with.
///
/// See [`read_ppid`] for why this does not open `/proc` itself. The environment at exec is the
/// right question: a process that calls `setenv` later did not change what its launcher handed it.
fn read_proc_env(pid: u32, key: &str) -> Option<String> {
    env_from_bytes(&sprag_terminal::procfs::environ(pid)?, key)
}

/// Find `key`'s value in a NUL-separated `KEY=VALUE` environ buffer (the pure core of
/// [`read_proc_env`], split out so it is testable without a live `/proc` entry).
fn env_from_bytes(bytes: &[u8], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    bytes
        .split(|&b| b == 0)
        .filter_map(|record| std::str::from_utf8(record).ok())
        .find_map(|record| record.strip_prefix(&prefix).map(str::to_owned))
}

/// The last `n` lines of `text` whose trimmed form is non-empty, in order.
fn last_n_lines(text: &str, n: usize) -> String {
    let kept: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = kept.len().saturating_sub(n);
    kept[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE COMPLETENESS RATCHET: every subject an agent is TOLD about, it can READ.**
    ///
    /// `wait_for_change` reports a change by naming its SUBJECT and nothing else — that is the
    /// event vocabulary's stated contract (*"an event names WHAT TO RE-READ; it does not carry the
    /// new value"*). A subject with no reader on this surface turns that contract into a dead end:
    /// the agent is told a window was renamed, or a session created, and has nowhere to go.
    ///
    /// **It was a dead end for two of the three subject kinds until R311**, measured against a real
    /// daemon: the roster had readers for PANE only, so an agent told `session_renamed work → prod`
    /// could read nothing about either name.
    ///
    /// Walked from `EventKind::ALL` rather than listed, so a kind added later that names a NEW
    /// subject fails here until its reader exists. That is the whole point — this is a ratchet, not
    /// a snapshot, and neither parity target has anything of the shape.
    #[test]
    fn every_subject_an_event_names_has_a_tool_that_reads_it() {
        let tools = tools_list();
        let names: Vec<&str> = tools["tools"]
            .as_array()
            .expect("a tool array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        // The reader declared for each subject key — the ONE place the pairing is stated, and the
        // match is exhaustive over the keys `EventKind::subject_key` can answer, so a fourth
        // subject added to the event vocabulary fails to COMPILE here rather than slipping through.
        let reader_for = |key: &str| -> &'static str {
            match key {
                k if k == sprag_host::events::Subject::PANE_KEY => "list_panes",
                k if k == sprag_host::events::Subject::WINDOW_KEY => "list_windows",
                k if k == sprag_host::events::Subject::SESSION_KEY => "list_sessions",
                // R355b's fourth subject. A run is not part of the containment — it DRIVES panes —
                // so its reader is the one that answers about runs, and an agent woken by
                // `run_finished` goes straight there for the outcome.
                k if k == sprag_host::events::Subject::RUN_KEY => "list_runs",
                other => panic!(
                    "the event vocabulary names a subject {other:?} that no tool on this surface \
                     reads — an agent told about it has nowhere to go. Add the reader, then name \
                     it here."
                ),
            }
        };

        let mut checked = 0;
        for kind in sprag_host::events::EventKind::ALL {
            let Some(key) = kind.subject_key() else {
                continue;
            };
            let reader = reader_for(key);
            assert!(
                names.contains(&reader),
                "`{}` names a {key} and this surface has no `{reader}` to read one",
                kind.wire_str(),
            );
            checked += 1;
        }
        // The COUNT, so a vocabulary that lost every subject-carrying kind could not pass this
        // vacuously — the shape R275 cost a round.
        assert!(
            checked >= 10,
            "only {checked} kinds carried a subject; this test asserted almost nothing",
        );

        // And the readers really are DISTINCT tools: three subjects answered by one tool would
        // satisfy every assertion above while leaving two of them unreadable.
        let readers = [
            sprag_host::events::Subject::PANE_KEY,
            sprag_host::events::Subject::WINDOW_KEY,
            sprag_host::events::Subject::SESSION_KEY,
        ]
        .map(reader_for);
        let unique: std::collections::HashSet<&str> = readers.into_iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "one tool cannot be three readers: {readers:?}"
        );
    }

    /// ⚠⚠ **EVERY SUBJECT AN EVENT NAMES IS RENDERED WITH IT** — the half the reader gate above
    /// could not see, and a LIVE defect when it was written.
    ///
    /// The gate above requires a TOOL that reads each subject. It says nothing about whether the
    /// agent is told WHICH one, and `render_events` matched three keys by hand: `run_finished`
    /// arrived, fell to the arm that prints the kind alone, and an agent with two loops in flight
    /// was told one had finished without being told which. The wait was woken and the answer was
    /// useless.
    ///
    /// Driven off `EventKind::ALL`, so a fifth subject fails here rather than being silently
    /// dropped — a hand-written match over subject keys is the list a new subject is left out of.
    #[test]
    fn every_subject_an_event_names_is_rendered_with_it() {
        let mut checked = 0;
        for kind in sprag_host::events::EventKind::ALL {
            let Some(key) = kind.subject_key() else {
                continue;
            };
            // A subject value of each shape the wire carries — an id for the two numeric keys, a
            // name for the two string ones. The VALUE is distinctive so its absence is visible.
            let value = if key == sprag_host::events::Subject::PANE_KEY
                || key == sprag_host::events::Subject::RUN_KEY
            {
                json!(4242)
            } else {
                json!("a-distinctive-name")
            };
            let line = render_events(&[json!({ "type": kind.wire_str(), key: value })], &[], &[]);
            assert!(
                line.contains("4242") || line.contains("a-distinctive-name"),
                "`{}` names a {key} and the rendering drops it — an agent is told something \
                 changed and not which: {line:?}",
                kind.wire_str(),
            );
            checked += 1;
        }
        assert!(
            checked >= 10,
            "only {checked} kinds carried a subject; this asserted almost nothing",
        );
    }

    #[test]
    fn the_wait_tool_names_every_change_the_daemon_can_report() {
        // ⚠ TWO REGISTER ITEMS, CLOSED BY ONE ASSERTION — and the first was live when this was
        // written: `pane_selected` was missing from the description while the daemon reported it, so
        // an agent could be woken by a change the tool had never told it existed, and `kinds:
        // ["pane_selected"]` looked like a word an agent had invented.
        //
        // R291 registered this as "the MCP tool description is the ONLY place the event vocabulary is
        // written out" — prose, in another crate, where no compiler or test could notice it drifting.
        // It is not prose-only any more: the list is derived from `EventKind::ALL` here, so a kind
        // added to the daemon fails this test until the surface an agent reads mentions it.
        let description = tools_list()["tools"]
            .as_array()
            .expect("a tool array")
            .iter()
            .find(|tool| tool["name"] == "wait_for_change")
            .expect("the wait tool")["description"]
            .as_str()
            .expect("a description")
            .to_owned();

        for kind in sprag_host::events::EventKind::ALL {
            assert!(
                description.contains(kind.wire_str()),
                "the wait tool must name `{}` — an agent cannot ask for, or make sense of, a change \
                 this description does not mention",
                kind.wire_str(),
            );
        }

        // And the SECOND item: the sampling ceiling was spelled "about 5 seconds" in English, a third
        // spelling of `SWEEP_INTERVAL` after the const and the sweep's own docs. The tool table is a
        // `json!` literal so it cannot interpolate a const, but a test can hold the two together.
        let seconds = sprag_host::agent::SWEEP_INTERVAL.as_secs();
        assert!(
            description.contains(&format!("{seconds} seconds")),
            "the description states the sampling delay as `{seconds} seconds`, which is \
             SWEEP_INTERVAL — if that constant moves, this sentence is the one nothing else would \
             correct",
        );
    }

    /// The roster's own names, in the order `tools/list` advertises them.
    fn advertised_tools() -> Vec<String> {
        tools_list()["tools"]
            .as_array()
            .expect("the roster is an array")
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .expect("every tool has a name")
                    .to_owned()
            })
            .collect()
    }

    /// **THIS ROSTER IS A PROJECTION OF [`sprag_host::vocabulary`], NOT A FOURTH CATALOGUE.**
    ///
    /// R323 joined the CLI, the keyboard and `--help` into one table and left this surface out of
    /// the join; the register's item 56 measured what that cost. This is the join, and it is held in
    /// BOTH directions on purpose:
    ///
    /// * every tool this file advertises is declared by some verb — so a tool added here without a
    ///   verb re-opens the fourth catalogue and fails;
    /// * every tool a verb declares is advertised here — so a verb cannot claim a tool that does not
    ///   exist, which is what makes [`Verb::tools`] safe for `sprag`'s own error sentences to print.
    ///
    /// It replaced a hard-written array of 29 names. That array was a ratchet — it would have caught
    /// a deletion — but it was also, exactly, a fifth list: it could agree with this file forever
    /// while the vocabulary said something else entirely, which is the state R335 found.
    #[test]
    fn the_roster_is_exactly_what_the_vocabulary_declares() {
        let advertised = advertised_tools();
        let declared: Vec<&str> = Verb::ALL
            .iter()
            .flat_map(|verb| verb.tools().iter().copied())
            .collect();
        for tool in &advertised {
            assert!(
                declared.contains(&tool.as_str()),
                "the roster advertises {tool:?} and no verb of sprag_host::vocabulary declares it \
                 — a tool nothing else in the product knows about is the fourth catalogue this \
                 ratchet exists to close",
            );
        }
        for tool in &declared {
            let verb = Verb::ALL
                .iter()
                .find(|verb| verb.tools().contains(tool))
                .expect("the name came from a verb");
            assert!(
                advertised.contains(&(*tool).to_owned()),
                "{} declares the tool {tool:?} and this roster does not advertise it, so an agent \
                 reading `sprag --help`'s vocabulary would ask for something that is not there",
                verb.name(),
            );
        }
        assert_eq!(
            advertised.len(),
            declared.len(),
            "the roster advertises a name twice",
        );
        // THE CONTROL: this is not vacuously true over two empty lists. The count is asserted where
        // the register's estimate was, so a later round reads a measured number.
        assert_eq!(advertised.len(), 39, "the agent surface's roster");
    }

    /// ⚠⚠ **EVERY `int` ARGUMENT OF A RUN IS CLASSIFIED AS A PANE OR AS NOT ONE** — the gate that
    /// makes [`PANE_ARGUMENTS`] fail-closed instead of being a list somebody remembered to update.
    ///
    /// The published grammar says `pane`, `src`, `dst`, `cols` and `max_iterations` are all `int`.
    /// Only the first three name a pane, and only a pane argument gets [`require_own_pane`]. So an
    /// `int` argument added upstream and left out of both lists would arrive UNRESOLVED and
    /// UNAUTHORISED — an agent could drive a pane it does not own by whichever new key that is.
    ///
    /// Two lists and this claim make the omission impossible: a new argument fails here, and the
    /// round that adds it decides which it is.
    #[test]
    fn every_int_argument_of_a_run_is_classified() {
        let mut seen = 0;
        for arg in orchestrate_arguments() {
            if arg.ty != "int" {
                continue;
            }
            seen += 1;
            assert!(
                PANE_ARGUMENTS.contains(&arg.name) ^ NOT_A_PANE.contains(&arg.name),
                "the run argument {:?} is an int that is in neither PANE_ARGUMENTS nor NOT_A_PANE \
                 (or in both). An unclassified int is one this tool passes through without \
                 resolving it or checking who owns it.",
                arg.name,
            );
        }
        assert_eq!(
            seen, 10,
            "the int arguments of every published run form: pane, src, dst, timeout_ms, cols, \
             rows, max_iterations, max_seconds, max_bytes and max_tokens",
        );
        // ⚠ AND THE EXEMPTION LIST IS PRUNED TOO — an entry naming an argument no form publishes
        // any more is a stale decision, which is the half R353's exemption rule adds.
        for name in PANE_ARGUMENTS.iter().chain(NOT_A_PANE) {
            assert!(
                orchestrate_arguments()
                    .iter()
                    .any(|arg| arg.name == *name || *name == OPENED_BY),
                "{name:?} is classified here and no run form publishes it",
            );
        }
    }

    /// ⚠⚠ **THE `orchestrate` SCHEMA IS THE WIRE'S OWN GRAMMAR, MINUS THE ONE ARGUMENT AN AGENT
    /// MAY NOT SEND.**
    ///
    /// Both halves are the claim. Every argument the daemon publishes is offered, so a plugin
    /// added upstream is callable here without an edit; and `opened_by` is NOT, because an agent
    /// that could stamp who asked for a run could claim — and then cancel — another pane's.
    #[test]
    fn the_orchestrate_schema_is_the_wires_own_grammar() {
        let schema = orchestrate_schema();
        let properties = schema["properties"]
            .as_object()
            .expect("an object of arguments");
        for form in run_forms() {
            for arg in form.args {
                for arg in std::iter::once(arg).chain(arg.fields) {
                    if !arg.fields.is_empty() {
                        continue; // the parent is offered by its fields
                    }
                    if arg.name == OPENED_BY {
                        assert!(
                            !properties.contains_key(OPENED_BY),
                            "the schema offers the provenance an agent must not choose",
                        );
                        continue;
                    }
                    assert!(
                        properties.contains_key(arg.name),
                        "the daemon publishes {:?} and the tool does not offer it, so an agent \
                         cannot send an argument this build's own wire takes",
                        arg.name,
                    );
                }
            }
        }
        // The DISCRIMINATOR carries every plugin word, so a fifth plugin is advertised the day it
        // is added rather than the day somebody edits a literal here.
        assert_eq!(
            properties["plugin"]["enum"],
            json!(sprag_host::plugins::PluginName::WIRE_WORDS),
        );
        assert_eq!(schema["required"], json!(["plugin"]));
        // THE CONTROL: the walk above is not vacuous — it really did visit the nested guardrail
        // fields, which is where the loop's whole safety story lives.
        for bound in ["max_iterations", "max_bytes", "max_tokens"] {
            assert!(
                properties.contains_key(bound),
                "{bound} is a nested field of the published grammar and must reach the agent",
            );
        }
    }

    /// ⚠⚠ **AN AGENT CANNOT SAY WHO ASKED FOR A RUN** — the authority decision, driven.
    ///
    /// This is refused before anything is sent, so it needs no daemon: the point is that the
    /// provenance is the SERVER's to stamp. Without it, `list_runs` and `cancel_run` would answer
    /// about whichever pane the caller claimed to be, and the ownership rule would be a suggestion.
    #[test]
    fn an_agent_cannot_stamp_who_asked_for_a_run() {
        let refusal = tool_orchestrate(&json!({
            "plugin": "agent", "pane": 1, "prompt": "hi", OPENED_BY: 99,
        }))
        .expect_err("a caller-supplied provenance is refused");
        assert!(refusal.contains(OPENED_BY), "{refusal}");
        // THE CONTROL: the refusal is about THAT key and not about the call being malformed in
        // general — an unknown key gets its own sentence naming what the tool does take.
        let other = tool_orchestrate(&json!({ "plugin": "agent", "nonsense": 1 }))
            .expect_err("an unknown argument is refused");
        assert!(
            other.contains("nonsense") && other.contains("prompt"),
            "{other}"
        );
    }

    #[test]
    fn tools_list_advertises_every_tool_with_object_schemas() {
        let tools = tools_list();
        for tool in tools["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 10);
        }
        // Required-argument spot checks, looked up BY NAME: an index would silently move to a
        // different tool the next time one is inserted above it (which is exactly what adding
        // `find_in_pane` did to the old `[5]`).
        let required = |name: &str| {
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name} is advertised"))["inputSchema"]["required"]
                .clone()
        };
        // write_pane requires pane + text (the "type xxx into pane 2" path).
        assert_eq!(required("write_pane"), json!(["pane", "text"]));
        // find_in_pane requires the pane AND something to look for.
        assert_eq!(required("find_in_pane"), json!(["pane", "needle"]));
        // regex_in_pane names its argument `pattern`, not `needle`: the argument NAME is part of
        // how an agent learns which language it is writing in.
        assert_eq!(required("regex_in_pane"), json!(["pane", "pattern"]));
        // agent_explain requires the pane it is explaining...
        assert_eq!(required("agent_explain"), json!(["pane"]));
        // A WINDOW is addressed by NAME and only by name — there is no number and no id on this
        // surface, so `window` is the whole grammar. `open_window` requires NOTHING: a window with
        // no name gets the lowest free number, exactly as `new-window` does for a person.
        assert_eq!(required("close_window"), json!(["window"]));
        assert_eq!(required("rename_window"), json!(["window", "name"]));
        assert_eq!(
            required("open_window"),
            json!(null),
            "a window an agent opens may be unnamed, like a person's",
        );
        // close_pane requires the pane to close; open_pane requires NOTHING, because the one thing
        // it must know — which pane is asking — is this server's own identity and never an argument
        // a caller could get wrong.
        assert_eq!(required("close_pane"), json!(["pane"]));
        assert_eq!(required("open_pane"), json!(null));
        // ...and `agent_state` requires NOTHING, which is the one asymmetry in this roster and is
        // deliberate: "which pane needs a human" is a question about the SET, so the whole-terminal
        // form is the one an agent asks first. A `required: ["pane"]` here would force it to ask
        // once per pane and assemble the answer itself.
        assert_eq!(required("agent_state"), json!(null));
        // `pane_layout` takes nothing either, and for its own reason: the arrangement is one thing
        // and the whole of it is the answer. A `pane` argument would make a caller ask per pane and
        // reassemble a shape the daemon already published in one piece.
        assert_eq!(required("pane_layout"), json!(null));
        // `select_pane` requires NOTHING and takes exactly one of two things — the daemon's own rule
        // for the same action. A JSON Schema cannot say "one of these two" without `oneOf`, which MCP
        // clients render poorly, so the constraint is stated in the description and enforced in the
        // tool; what a schema CAN do is publish the direction words, which is where an agent learns
        // them.
        assert_eq!(required("select_pane"), json!(null));
        let select = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "select_pane")
            .expect("select_pane is advertised")
            .clone();
        assert_eq!(
            select["inputSchema"]["properties"]["dir"]["enum"],
            json!(["left", "right", "up", "down"]),
            "the words come from PaneDir, so a direction the daemon gains cannot go unadvertised",
        );
        // And the honesty an agent needs most: a bare direction moves from where the USER is, not
        // from the agent's own pane. A caller that assumed otherwise would move a person somewhere
        // surprising and read the answer as agreement — so the default is stated in capitals, and
        // both ways of asking the OTHER question are named right beside it, because an agent that
        // learns the default without the remedy is left believing the question cannot be asked.
        let description = select["description"].as_str().unwrap();
        assert!(
            description.contains("steps FROM WHERE THE USER IS NOW"),
            "the tool must say whose position a bare direction is relative to: {description}",
        );
        for named in [SelectAsk::FROM_KEY, FROM_HERE_ARG] {
            assert!(
                description.contains(named),
                "the description must name '{named}', the argument that changes that: \
                 {description}",
            );
            assert!(
                select["inputSchema"]["properties"][named].is_object(),
                "and the schema must publish it, or no agent can send it",
            );
        }
        assert_eq!(
            select["inputSchema"]["properties"][SelectAsk::FROM_KEY]["type"],
            json!(["integer", "string"]),
            "an origin takes the same two handles a target does — a NUMBER or a NAME",
        );
        assert_eq!(
            select["inputSchema"]["properties"][FROM_HERE_ARG]["type"],
            json!("boolean"),
            "and the agent's OWN pane is a boolean, because both string and integer already mean \
             something else here",
        );
    }

    /// All four outcomes as an AGENT reads them, plus the two degradations — a live daemon can be
    /// driven into the first two and only awkwardly into the rest, which is why the rendering is a
    /// pure function.
    ///
    /// The two "nothing moved" sentences must not be interchangeable: their remedies are opposite
    /// (look at the arrangement / that pane is in no arrangement, so name one), and an agent that
    /// read one for the other would either keep pressing at an edge or keep waiting for a float to
    /// gain a neighbour.
    #[test]
    fn a_selection_reads_as_a_sentence_about_where_the_user_is_now() {
        assert_eq!(
            render_selection(
                SelectHow::Moved,
                None,
                None,
                Some("pane 2 (\"build\")"),
                11,
                None
            ),
            "The user is now on pane 2 (\"build\") — the active pane of this session.",
            "a NAME rides in the sentence, because that is the handle the caller can reuse",
        );
        assert_eq!(
            render_selection(
                SelectHow::Moved,
                Some(PaneDir::Left),
                None,
                Some("pane 1"),
                10,
                None
            ),
            "Moved the user one pane left: they are now on pane 1.",
        );
        assert_eq!(
            render_selection(
                SelectHow::AlreadyActive,
                None,
                None,
                Some("pane 2"),
                11,
                None
            ),
            "The user was already on pane 2; nothing moved.",
        );
        let edge = render_selection(
            SelectHow::AtEdge,
            Some(PaneDir::Up),
            None,
            Some("pane 1"),
            10,
            None,
        );
        assert!(
            edge.contains("There is nothing above pane 1")
                && edge.contains("still on it")
                && edge.contains("pane_layout"),
            "an edge names the direction, says nobody moved, and points at the read that \
             explains it: {edge}",
        );
        let floating = render_selection(
            SelectHow::Untiled,
            Some(PaneDir::Up),
            None,
            Some("pane 3"),
            12,
            None,
        );
        assert!(
            floating.contains("on pane 3, which is FLOATING")
                && floating.contains("no arrangement")
                && floating.contains("'pane'"),
            "a floating pane gets the OTHER remedy — name the pane you want: {floating}",
        );
        assert_ne!(edge, floating, "two outcomes, two sentences");

        // A pane that exited between the select and the listing that would name it: the move HAPPENED,
        // so the answer says so and hands back the id a caller can still look up — never an error,
        // which would send the caller to move a cursor that has already moved.
        let vanished =
            render_selection(SelectHow::Moved, Some(PaneDir::Right), None, None, 42, None);
        assert!(
            vanished.contains("id 42") && vanished.contains("list_panes"),
            "the fallback subject is the id, with the read that resolves it: {vanished}",
        );
    }

    /// All four SWAP outcomes as an agent reads them, plus the degradation —
    /// [`a_selection_reads_as_a_sentence_about_where_the_user_is_now`]'s rule one verb over.
    ///
    /// The two "nothing moved" sentences must not be interchangeable here either, and the remedies
    /// are the same pair: look the other way / there is no way to look. **Every success sentence
    /// says nobody's cursor moved**, because the verb one tool over does exactly that and an agent
    /// holding both needs to know which one it just called.
    #[test]
    fn a_swap_reads_as_a_sentence_about_where_the_pane_is_now() {
        let moved = render_swap(
            SwapHow::Swapped,
            Some(PaneDir::Left),
            "pane 3 (\"build\")",
            Some("pane 1"),
        );
        assert_eq!(
            moved,
            "Moved pane 3 (\"build\") one place left: it and pane 1 have traded places. Nobody's \
             cursor moved — call select_pane if you want the user to look at it.",
            "a NAME rides in the sentence, and so does the pane it traded with — the handle a \
             `dir` caller never typed",
        );
        assert_eq!(
            render_swap(SwapHow::Swapped, None, "pane 3", Some("pane 1")),
            "pane 3 and pane 1 have traded places. Nobody's cursor moved — call select_pane if \
             you want the user to look at it.",
        );
        assert_eq!(
            render_swap(SwapHow::SamePane, None, "pane 3", Some("pane 3")),
            "pane 3 is the pane you asked to trade it with, so nothing moved.",
        );
        let edge = render_swap(SwapHow::AtEdge, Some(PaneDir::Up), "pane 1", None);
        assert!(
            edge.contains("There is nothing above pane 1")
                && edge.contains("stayed where it is")
                && edge.contains("pane_layout"),
            "an edge names the direction, says nothing moved, and points at the read that \
             explains it: {edge}",
        );
        let floating = render_swap(SwapHow::Untiled, Some(PaneDir::Up), "pane 3", None);
        assert!(
            floating.contains("pane 3 is FLOATING")
                && floating.contains("no arrangement")
                && floating.contains("'with'"),
            "a floating pane gets the OTHER remedy — name the pane to trade with: {floating}",
        );
        assert_ne!(edge, floating, "two outcomes, two sentences");
        // A partner the listing no longer holds: the trade HAPPENED, so the answer says so rather
        // than failing a call that succeeded.
        assert!(
            render_swap(SwapHow::Swapped, Some(PaneDir::Right), "pane 3", None)
                .contains("the other pane"),
        );
    }

    /// **THE FOUR ZOOM SENTENCES, PINNED — and this test exists because its function's doc SAID it
    /// did before it was written.**
    ///
    /// R334's rule, broken in the round that recorded it: *a doc comment nothing can contradict is
    /// a claim, not a guarantee.* `render_zoom`'s doc read *"all four sentences are pinned by a unit
    /// test"* while `render_swap` was the only one of the three renderers that had one. The live
    /// gate does drive all four states against a real daemon, so the claim was nearly true and
    /// attributed to an instrument that did not exist — which is the shape, not a degree of it.
    ///
    /// What a unit test adds over that live gate is the WORDING: a rename of a phrase an agent is
    /// told to act on ("call zoom_pane with `on: false` when you are done") would leave the live
    /// gate green if it still matched its looser needle.
    #[test]
    fn the_four_zoom_sentences_each_say_which_state_they_left() {
        let said = |zoomed, changed| render_zoom("pane 2", zoomed, changed);
        assert!(
            said(true, true).contains("now fills its window")
                && said(true, true).contains("zoom_pane with `on: false`"),
            "a zoom that happened says so and says how to undo it: {}",
            said(true, true),
        );
        assert!(
            said(true, false).contains("was already filling its window; nothing moved"),
            "asking for the state it is in is not the same sentence: {}",
            said(true, false),
        );
        assert!(
            said(false, true).contains("no longer fills its window")
                && said(false, true).contains("visible again"),
            "an unzoom names what came back: {}",
            said(false, true),
        );
        assert!(
            said(false, false).contains("was not filling its window"),
            "and the fourth is its own sentence: {}",
            said(false, false),
        );
        // THE CONTROL: four DISTINCT sentences. Three arms collapsing onto one string would satisfy
        // every assertion above that happened to share a phrase.
        let mut all = [
            said(true, true),
            said(true, false),
            said(false, true),
            said(false, false),
        ];
        all.sort();
        let before = all.len();
        let mut unique = all.to_vec();
        unique.dedup();
        assert_eq!(before, unique.len(), "two zoom outcomes read identically");
        // Every one names its subject, because an agent may have several panes in flight.
        for sentence in &unique {
            assert!(sentence.starts_with("pane 2"), "{sentence:?}");
        }
    }

    /// The handle an answer hands back is one a caller can pass to the next tool — a NUMBER, plus the
    /// NAME when the pane has one, because that is the handle that survives a pane closing.
    ///
    /// And for a pane ONE WINDOW OVER it is the id and the window, never a number: a number there
    /// would be read straight back as `pane: N` and land on a different pane. The two rows below
    /// carry the SAME `PaneInfo`, so the only thing telling the handles apart is where the pane is.
    #[test]
    fn a_pane_is_named_back_by_the_handles_a_caller_can_use() {
        let mut info = parse_pane_info(&json!({ "id": 11, "cols": 80, "rows": 24 }));
        let near = |info: &PaneInfo| PaneRef {
            number: Some(2),
            window: None,
            info: info.clone(),
        };
        let far = |info: &PaneInfo| PaneRef {
            number: None,
            window: Some("build".to_owned()),
            info: info.clone(),
        };
        assert_eq!(render_pane_handle(&near(&info)), "pane 2");
        assert_eq!(render_pane_handle(&far(&info)), "pane id 11 (window build)");
        info.name = Some("tests".to_owned());
        assert_eq!(render_pane_handle(&near(&info)), "pane 2 (\"tests\")");
        assert_eq!(
            render_pane_handle(&far(&info)),
            "pane id 11 (window build) (\"tests\")",
            "a far pane is named by the two handles that reach it, never by a number",
        );
    }

    #[test]
    fn parse_image_info_reads_a_summary_and_rejects_a_malformed_one() {
        let img = parse_image_info(&json!({
            "id": 7, "width": 640, "height": 480, "anchor": [3, 10], "seq": 2
        }))
        .expect("a well-formed summary parses");
        assert_eq!(
            (img.id, img.width, img.height, img.col, img.row),
            (7, 640, 480, 3, 10)
        );
        // A missing anchor is dropped (None), never a torn half-summary.
        assert!(
            parse_image_info(&json!({ "id": 1, "width": 2, "height": 2 })).is_none(),
            "a summary missing its anchor is rejected"
        );
    }

    #[test]
    fn initialize_echoes_the_clients_protocol_version_and_names_the_server() {
        let req = json!({ "params": { "protocolVersion": "2030-01-01" } });
        let result = handle_initialize(&req);
        assert_eq!(result["protocolVersion"], "2030-01-01");
        assert_eq!(result["serverInfo"]["name"], "sprag-mcp");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("pane of a sprag terminal")
        );
    }

    /// The open answer PROMISES the cleanup only when the terminal really recorded the provenance.
    ///
    /// The false branch is not defensive noise, and it is not hypothetical: the skew run for this
    /// change (a parent-commit daemon, same wire protocol 4) accepted `opened_by` and recorded
    /// nothing, because the field did not exist yet. Reading the fact BACK off the pane is what
    /// turns that into a warning instead of a promise this tool would go on to break — and the
    /// warning is here rather than live because the suite can only build one daemon.
    #[test]
    fn the_open_answer_only_promises_a_close_it_can_keep() {
        let kept = opened_answer("2", " in /tmp", true, None, "LISTING");
        assert_eq!(
            kept,
            "Opened pane 2 in /tmp, running a shell. It is recorded as opened by your pane, so \
             close_pane will let you close it.\n\nLISTING",
        );
        let broken = opened_answer("2", "", false, None, "LISTING");
        assert!(
            broken.starts_with(
                "Opened pane 2, running a shell. WARNING: this terminal did not record it as \
                 opened by you, so close_pane will refuse it"
            ),
            "an older daemon dropped the fact, and the answer says so rather than promising: \
             {broken}",
        );
    }

    /// And the answer only tells a caller to USE a name the terminal really recorded.
    ///
    /// The same shape one field over, and it is here rather than live for the same reason. The
    /// asymmetry it guards is the one R294's skew run established: an additive FIELD an old client
    /// ignores is harmless, while an additive ARGUMENT an old DAEMON ignores is a silent no-op —
    /// so a tool that told the caller "address it as \"build\"" from its own request would hand out
    /// a handle that resolves to nothing.
    #[test]
    fn the_open_answer_only_offers_a_name_the_terminal_recorded() {
        let landed = opened_answer("2", "", true, Some(("build", true)), "LISTING");
        assert!(
            landed.contains(
                "It is called \"build\" — pass that as `pane` instead of the number, which will \
                 shift if an earlier pane closes."
            ),
            "the name is offered WITH the reason to use it: {landed}",
        );
        let dropped = opened_answer("2", "", true, Some(("build", false)), "LISTING");
        assert!(
            dropped.contains(
                "WARNING: this terminal did not record the name \"build\", so addressing the pane \
                 by it will fail"
            ),
            "an older daemon dropped the argument, and the answer says so: {dropped}",
        );
        assert!(
            !opened_answer("2", "", true, None, "LISTING").contains("called"),
            "and a caller that asked for no name is told nothing about one",
        );
    }

    /// The primer an agent reads before anything else NAMES every tool this server advertises.
    ///
    /// Derived from the roster rather than checked against a written list, because a written list
    /// is what had already gone wrong — TWICE, silently. The primer taught "read, ask about the AI,
    /// type, send keys" long after `pane_processes` (what a pane is RUNNING) and `wait_for_change`
    /// (do not poll) had shipped, so an agent's first and most-read description of this surface
    /// omitted the two tools that most change how it should behave. Nothing failed, because nothing
    /// compared the two.
    ///
    /// Exactly R292's fix for the same hazard one level down (the wait tool's change list, derived
    /// from `EventKind::ALL` after it was found missing `pane_selected`). A search that finds one
    /// instance of a hazard has found the hazard: this is the second instance, in the same file.
    #[test]
    fn the_primer_names_every_tool_the_server_advertises() {
        let primer = handle_initialize(&json!({ "params": {} }))["instructions"]
            .as_str()
            .expect("the server hands the agent a primer")
            .to_owned();
        let tools = tools_list();
        let missing: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .filter(|name| !primer.contains(&format!("`{name}`")))
            .collect();
        assert!(
            missing.is_empty(),
            "the primer never mentions {missing:?}, so an agent reading it would not know those \
             tools exist: {primer}",
        );
    }

    #[test]
    fn initialize_falls_back_to_the_default_version_when_absent() {
        let result = handle_initialize(&json!({ "params": {} }));
        assert_eq!(result["protocolVersion"], DEFAULT_PROTOCOL_VERSION);
    }

    #[test]
    fn a_tool_error_is_content_with_is_error_not_a_protocol_error() {
        // send_keys with no 'keys' is a business error -> isError content.
        let call = json!({ "params": { "name": "send_keys", "arguments": { "pane": 1 } } });
        let result = handle_tools_call(&call);
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Error:")
        );
    }

    #[test]
    fn unknown_tool_is_reported_as_an_error_content() {
        let call = json!({ "params": { "name": "nope", "arguments": {} } });
        let result = handle_tools_call(&call);
        assert_eq!(result["isError"], true);
    }

    /// A reading of one pane, built the way the daemon serves it.
    fn reading(rows: Vec<sprag_terminal::PaneProcesses>) -> PaneProcessesWire {
        PaneProcessesWire {
            sampled_ms_ago: 7,
            panes: rows,
        }
    }

    fn row(
        id: u64,
        tty: Option<&str>,
        shell_pid: Option<u32>,
        foreground: Option<sprag_terminal::ForegroundJob>,
    ) -> sprag_terminal::PaneProcesses {
        sprag_terminal::PaneProcesses {
            id,
            tty: tty.map(str::to_owned),
            shell_pid,
            foreground,
        }
    }

    fn job(pgid: u32, processes: Vec<sprag_terminal::JobProcess>) -> sprag_terminal::ForegroundJob {
        sprag_terminal::ForegroundJob { pgid, processes }
    }

    fn process(pid: u32, name: &str, argv: &[&str]) -> sprag_terminal::JobProcess {
        sprag_terminal::JobProcess {
            pid,
            name: name.to_owned(),
            argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    /// The whole answer an agent receives, pinned as TEXT — the numbering, the device, the job, and
    /// the argv quoting.
    ///
    /// The quoting is the load-bearing part: the wire carries an argument VECTOR precisely so that
    /// `git commit -m 'two words'` cannot be confused with a four-argument command, and this is the
    /// one place that has to flatten it. A renderer that joined with bare spaces would print the two
    /// commands below identically, so both are here.
    #[test]
    fn the_process_answer_names_each_pane_and_quotes_every_argument() {
        let wire = reading(vec![
            row(
                40,
                Some("/dev/pts/3"),
                Some(900),
                Some(job(900, vec![process(900, "bash", &["/bin/bash"])])),
            ),
            row(
                41,
                Some("/dev/pts/4"),
                Some(901),
                Some(job(
                    950,
                    vec![
                        process(950, "git", &["git", "commit", "-m", "two words"]),
                        process(951, "less", &[]),
                    ],
                )),
            ),
        ]);

        assert_eq!(
            render_processes_answer(
                &wire,
                &pool(&[40, 41]),
                &elsewhere("0", &[40, 41]),
                &[],
                None,
            ),
            "What each pane is running, sampled 7 ms ago:\n\
             \n\
             pane 1 (id 40) on /dev/pts/3, child process 900\n\
             \x20 running (job 900):\n\
             \x20   900 bash  /bin/bash\n\
             pane 2 (id 41) on /dev/pts/4, child process 901\n\
             \x20 running (job 950):\n\
             \x20   950 git  git commit -m 'two words'\n\
             \x20   951 less  (no command line)\n\
             \n\
             The command in list_panes is what a pane was SPAWNED with and never changes; this is \
             what it is running now. A pane whose job is its own child process is sitting at a \
             prompt.\n",
        );
    }

    /// The three states a pane can be in that are NOT "running something", each said distinctly:
    /// a reaped child, a live child whose terminal nobody owns, and a row the pane list no longer
    /// names. Collapsing any pair of them would hide a real difference.
    #[test]
    fn a_pane_with_no_job_says_which_kind_of_nothing_it_is() {
        let wire = reading(vec![
            row(40, Some("/dev/pts/3"), None, None),
            row(41, Some("/dev/pts/4"), Some(901), None),
            row(99, None, Some(902), None),
        ]);
        let answer = render_processes_answer(
            &wire,
            &pool(&[40, 41]),
            &elsewhere("0", &[40, 41]),
            &[],
            None,
        );

        assert!(
            answer.contains("pane 1 (id 40) on /dev/pts/3 — no child process\n"),
            "a reaped child is named as one: {answer}",
        );
        assert!(
            answer.contains(
                "pane 2 (id 41) on /dev/pts/4, child process 901\n  nothing owns this pane's \
                 terminal\n"
            ),
            "a live child with an unowned terminal is a different state: {answer}",
        );
        assert!(
            answer.contains("pane ? (id 99, gone since the pane list was read), child process 902"),
            "and a row the numbering cannot name says so rather than borrowing a number: {answer}",
        );
    }

    /// Narrowing to one pane answers about that pane and no other — and it narrows by NUMBER
    /// against the pane list, so an unnameable row cannot be selected by accident.
    #[test]
    fn a_narrowed_process_answer_holds_one_pane() {
        let wire = reading(vec![
            row(40, None, Some(900), None),
            row(41, None, Some(901), None),
        ]);
        let here = pool(&[40, 41]);
        let answer = render_processes_answer(
            &wire,
            &here,
            &elsewhere("0", &[40, 41]),
            &[],
            Some(&near(2, here[1].clone())),
        );
        assert!(answer.contains("pane 2 (id 41)"), "{answer}");
        assert!(!answer.contains("pane 1 (id 40)"), "{answer}");
    }

    /// **A LIVE PANE ONE WINDOW OVER IS NOT "GONE", and this is the sentence that said it was.**
    ///
    /// The reading is REGISTRY-wide and the numbering is one window's, so before R312 every row
    /// belonging to another window was rendered *"pane ? (id N, gone since the pane list was
    /// read)"* — measured against a real two-window daemon, in the same line that went on to report
    /// that pane's tty and its child's pid. The residual sentence was written for a genuine race
    /// and had come to fire for the ordinary case.
    ///
    /// The fixture is chosen so the three answers actually DISAGREE: `41` is in another window and
    /// `99` is in none, so a renderer that looked in one listing would call them the same thing.
    #[test]
    fn a_row_from_another_window_names_that_window_instead_of_calling_it_gone() {
        let wire = reading(vec![
            row(40, None, Some(900), None),
            row(41, None, Some(901), None),
            row(99, None, Some(902), None),
        ]);
        let answer =
            render_processes_answer(&wire, &pool(&[40]), &elsewhere("build", &[41]), &[], None);
        assert!(
            answer.contains("pane 1 (id 40)"),
            "your own window still numbers its panes: {answer}",
        );
        assert!(
            answer.contains("pane id 41 (window build)"),
            "a pane one window over is named by where it IS: {answer}",
        );
        assert!(
            answer.contains("pane ? (id 99, gone since the pane list was read)"),
            "and the residual sentence is kept for the row that really is gone: {answer}",
        );
        assert!(
            !answer.contains("pane ? (id 41"),
            "the live pane one window over must not be reported as gone: {answer}",
        );
    }

    /// A pane list of `n` panes whose host ids are deliberately NOT their numbers — the mapping this
    /// surface exists to keep straight, and one an off-by-anything would pass with ids of `1..=n`.
    fn pool(ids: &[u64]) -> Vec<PaneInfo> {
        ids.iter()
            .map(|id| parse_pane_info(&json!({ "id": id })))
            .collect()
    }

    /// Panes of ANOTHER window, paired with its name — the second listing every row-naming answer
    /// now consults, so a live pane one window over is not reported as gone.
    fn elsewhere(window: &str, ids: &[u64]) -> Vec<(String, PaneInfo)> {
        pool(ids)
            .into_iter()
            .map(|pane| (window.to_owned(), pane))
            .collect()
    }

    /// Panes of ANOTHER SESSION, as the registry-wide tree publishes them — the third listing, and
    /// the one that stops a live pane on somebody else's session being called gone.
    fn other_session(name: &str, ids: &[u64]) -> Vec<sprag_terminal::TreeSession> {
        vec![sprag_terminal::TreeSession {
            id: sprag_terminal::SessionId(9),
            name: name.to_owned(),
            default: false,
            attached: 0,
            windows: vec![sprag_terminal::TreeWindow {
                id: sprag_terminal::WindowId(1),
                name: "0".to_owned(),
                current: true,
                panes: ids
                    .iter()
                    .map(|id| sprag_terminal::TreePane {
                        id: sprag_terminal::PaneId(*id),
                        name: None,
                        command: "bash".to_owned(),
                        active: false,
                    })
                    .collect(),
            }],
        }]
    }

    /// One pane's resource row, for the renderer tests.
    fn taken(id: u64, millicores: u64, waiting_hundredths: u32) -> sprag_terminal::PaneResources {
        sprag_terminal::PaneResources {
            id,
            taken: sprag_terminal::Taken::Measured {
                cpu: sprag_terminal::Cpu::Held {
                    millicores,
                    over_ms: 1000,
                },
                waiting: sprag_terminal::Waiting::Measured {
                    avg10: sprag_terminal::Percent::from_hundredths(waiting_hundredths),
                    avg60: sprag_terminal::Percent::NONE,
                    avg300: sprag_terminal::Percent::NONE,
                },
                memory: sprag_terminal::Counted::Now(6 * 1024 * 1024),
                processes: sprag_terminal::Counted::Now(5),
                // A REAL grant, not an absent one: a fixture whose every ceiling is missing cannot
                // express the row this surface exists to print, and a renderer test written over it
                // would pass against a renderer that dropped the grant entirely.
                granted: sprag_terminal::Granted {
                    share: sprag_terminal::Counted::Now(100),
                    memory: sprag_terminal::Ceiling::At(512 * 1024 * 1024),
                    processes: sprag_terminal::Ceiling::At(64),
                },
            },
        }
    }

    fn resource_reading(rows: Vec<sprag_terminal::PaneResources>) -> PaneResourcesWire {
        PaneResourcesWire {
            sampled_ms_ago: 7,
            panes: rows,
        }
    }

    /// **A LIVE PANE ONE SESSION OVER IS NOT "GONE" EITHER** — R312's fix, finished.
    ///
    /// R312 taught this sentence about another WINDOW and left it wrong about another SESSION,
    /// where nothing exercised it because no registry-wide tool had a reason to name one. R338's
    /// does: `pane_resources` answers about the MACHINE, so the pane taking every core is very often
    /// in a session the caller is not in — and it was rendered *"gone since the pane list was
    /// read"* on a live daemon, in the same line that went on to report its 19 cores.
    ///
    /// The fixture makes the three answers DISAGREE: `41` is one window over, `77` is one session
    /// over, `99` is nowhere.
    #[test]
    fn a_row_from_another_session_names_that_session_instead_of_calling_it_gone() {
        let wire = resource_reading(vec![
            taken(40, 100, 0),
            taken(41, 200, 0),
            taken(77, 19_000, 5011),
            taken(99, 0, 0),
        ]);

        let answer = render_resources_answer(
            &wire,
            &pool(&[40]),
            &elsewhere("build", &[41]),
            &other_session("work", &[77]),
            None,
        );

        assert!(
            answer.contains("pane 1 (id 40)"),
            "your own window still numbers its panes: {answer}",
        );
        assert!(
            answer.contains("pane id 41 (window build)"),
            "a pane one window over is named by its window: {answer}",
        );
        assert!(
            answer.contains("pane id 77 (session work)"),
            "a pane one SESSION over is named by its session, not called gone: {answer}",
        );
        assert!(
            answer.contains("pane ? (id 99, gone since the pane list was read)"),
            "and the race the last arm is for still says so: {answer}",
        );
    }

    /// The two numbers arrive TOGETHER, and the window each rate covers is stated.
    ///
    /// The whole argument for this tool: cores held cannot be read without the waiting figure beside
    /// it, and a rate cannot be read without the window it covers. A renderer that printed either
    /// alone would be handing an agent a number it cannot act on.
    #[test]
    fn a_resource_row_states_what_it_got_what_it_waited_for_and_over_how_long() {
        let wire = resource_reading(vec![taken(40, 3590, 774)]);

        let answer = render_resources_answer(&wire, &pool(&[40]), &[], &[], None);

        assert!(
            answer.contains("holding 3.59 CPU cores, measured over the last 1000 ms"),
            "{answer}"
        );
        assert!(
            answer.contains("waiting 7.74% of the last 10 seconds"),
            "{answer}"
        );
        // The usage AND its ceiling, in one row. A usage alone is not a fact an agent can act on:
        // `6 MiB` is only meaningful once it is `6 MiB of 512 MiB`, and asserting the bare usage
        // would pass just as well against a renderer that dropped the grant entirely.
        assert!(
            answer.contains("6 MiB of memory, of a ceiling of 512 MiB"),
            "{answer}"
        );
        assert!(
            answer.contains("5 processes, of a ceiling of 64"),
            "{answer}"
        );
        // And the weight, with the warning that makes it readable — a weight rendered as a share of
        // the machine is the one thing this must never say.
        assert!(answer.contains("a CPU weight of 100"), "{answer}");
        assert!(
            answer.contains("a weight is not a cap and not a ratio"),
            "{answer}"
        );
    }

    /// Every shape of the GRANT sentences an agent reads — the shell's
    /// `every_grant_column_says_which_of_its_shapes_it_is`, in this register.
    ///
    /// Two registers for one fact, gated on both sides, which is the rule this pair already
    /// follows for the resource columns. What must never differ is the MEANING, and the place it
    /// could slip is the absences: an agent told "no ceiling" when the truth is "this host cannot
    /// hold one" would try again with a smaller number forever.
    #[test]
    fn every_grant_sentence_says_which_of_its_shapes_it_is() {
        // Beside a usage. An UNCAPPED pane says so out loud here, unlike the shell's column — an
        // agent deciding whether a sibling can be told to use less needs to know that nothing is
        // stopping it, and silence would read as "there is a ceiling and I did not mention it".
        assert_eq!(
            agent_of(
                "6 MiB of memory".to_owned(),
                Ceiling::At(512 * 1024 * 1024),
                agent_bytes
            ),
            "6 MiB of memory, of a ceiling of 512 MiB",
        );
        assert_eq!(
            agent_of("6 MiB of memory".to_owned(), Ceiling::Uncapped, agent_bytes),
            "6 MiB of memory, with no ceiling set",
        );
        assert_eq!(
            agent_of(
                "memory unmeasured".to_owned(),
                Ceiling::NoController,
                agent_bytes
            ),
            "memory unmeasured",
            "the usage has already named the missing controller",
        );

        // ALONE, on `grant_pane`'s answer, the missing controller names WHOSE problem it is.
        assert_eq!(agent_ceiling(Ceiling::At(64), agent_count_ceiling), "64");
        assert_eq!(
            agent_ceiling(Ceiling::Uncapped, agent_count_ceiling),
            "no ceiling"
        );
        let blind = agent_ceiling(Ceiling::NoController, agent_count_ceiling);
        assert!(
            blind.contains("delegation"),
            "an agent is told the host cannot hold a ceiling, not that there is none: {blind}",
        );
        assert_ne!(
            agent_ceiling(Ceiling::Uncapped, agent_bytes),
            agent_ceiling(Ceiling::NoController, agent_bytes),
        );

        // THE WEIGHT, and the warning that makes it safe to act on. An agent told a weight without
        // this would compute a share of the machine from it; a nominal 10:100 measured 18:82, and a
        // cgroup weighted 10 took every core it was offered once its sibling went idle.
        let weight = agent_weight(Counted::Now(10));
        assert!(weight.contains("weight of 10"), "{weight}");
        assert!(
            weight.contains("not a cap and not a ratio"),
            "the weight never travels without the sentence that stops it being read as a share: \
             {weight}",
        );
        assert!(
            !weight.contains('%'),
            "and nothing shaped like a percentage: {weight}"
        );
        assert!(
            agent_weight(Counted::NoController).contains("no cpu controller"),
            "a host with no cpu delegation says so rather than reporting a weight of zero",
        );
    }

    /// ⚠⚠ EVERY CHECK REACHES THE AGENT — the claim `render_health_answer` had NO test for at all.
    ///
    /// R339 built this renderer and R342 added a check to the set it renders; between them nothing
    /// asserted that a check the daemon judged is a check the agent is TOLD about. Measured:
    /// dropping one row from the loop below left the whole crate GREEN, so the agent surface for
    /// layer 2 could have silently lost any row at any time.
    ///
    /// Both halves, because they are different code paths: a DEGRADED check is printed by the
    /// first loop with its criterion and remedy, and everything else by the second. A gate on one
    /// says nothing about the other — which is the shape this project keeps meeting at doors.
    #[test]
    fn every_check_the_daemon_judged_reaches_the_agent() {
        use sprag_terminal::doctor::{Blind, Evidence, Finding, Verdict};

        // One report holding every check, alternating the three verdicts so that both loops run
        // and neither is empty. Built from `Check::ALL`, so a check added to the set is in this
        // test the day it compiles.
        let findings: Vec<Finding> = sprag_terminal::Check::ALL
            .iter()
            .enumerate()
            .map(|(nth, check)| Finding {
                check: *check,
                verdict: match nth % 3 {
                    0 => Verdict::Degraded,
                    1 => Verdict::Healthy,
                    _ => Verdict::Blind(Blind::NoPanes),
                },
                evidence: Evidence::of("what it read", format!("reading {nth}")),
            })
            .collect();
        let answer = render_health_answer(&Diagnosis {
            findings: findings.clone(),
        });

        // ⚠ MATCHED AS A ROW, NOT AS A SUBSTRING. The first version of this asked
        // `answer.contains(entry.name)` and was GREEN while `pane-admission` was deleted from the
        // renderer — because `pane-isolation`'s REMEDY names that row in its prose. A probe whose
        // pattern appears in text it did not come from answers about the wrong thing (R253/R338/
        // R339, and now this test). Every row here is printed as `<name> — ...` at a line start,
        // so that is what is asserted.
        for finding in &findings {
            let entry = finding.check.entry();
            let row = format!("{} — ", entry.name);
            assert!(
                answer.lines().any(|line| line.starts_with(&row)),
                "{:?} was judged {:?} and the agent gets no row for it: {answer}",
                finding.check,
                finding.verdict,
            );
        }
        assert!(
            answer.contains("reading"),
            "and the rows arrive with what they measured: {answer}",
        );
        // The counts in the opening sentence are the report's, not a hand-tallied number.
        assert!(
            answer.contains(&format!("of {} checks", findings.len())),
            "the agent is told how many checks there were: {answer}",
        );
        // A degraded row carries its remedy; a clean one must not pretend to have been checked.
        assert!(
            answer.contains("do not do it yourself"),
            "a remedy is named as the PERSON's to run: {answer}",
        );
        assert!(
            answer.contains("not measurable"),
            "and a blind row says so rather than reading as healthy: {answer}",
        );
    }

    /// A pane with no reading says WHICH reason it is, never a blank or a zero.
    ///
    /// Each is acted on differently — a whole machine that enforces nothing, one pane nobody placed,
    /// one the KERNEL turned away, and one that ended — and an agent that read "0 cores" for any of
    /// them would conclude the pane was idle.
    ///
    /// ⚠ **THE EXPECTED TEXT COMES FROM A MATCH AND NOT FROM THE LIST BESIDE IT.** The first
    /// version of this test paired each reason with its words in one hand-written array, and when
    /// R342 added [`Unmeasured::Refused`](sprag_terminal::Unmeasured::Refused) the array simply did
    /// not mention it: the new arm reached an agent's screen with nothing asserting what it said.
    /// A match cannot do that — a new arm stops this file compiling until somebody writes down
    /// what an agent will read.
    ///
    /// ⚠ And the LIST is `Unmeasured::ALL`, not an array retyped here. The match forces a sentence
    /// to be written down for a new arm; only the closed set forces the new arm to be RUN. Both
    /// halves are needed and this test had neither — which is how the fourth reason shipped past
    /// it. `Unmeasured` became a closed set in the same round for exactly this reason.
    #[test]
    fn an_unmeasured_pane_says_which_reason_it_is() {
        for reason in sprag_terminal::Unmeasured::ALL {
            let said = match reason {
                sprag_terminal::Unmeasured::NothingEnforced => "no cgroup subtree",
                sprag_terminal::Unmeasured::NotPlaced => "never placed",
                // The kernel's own sentence, not a paraphrase: it is what a person searching for
                // why their panes are unweighted will actually have in front of them.
                sprag_terminal::Unmeasured::Refused(_) => "would not admit",
                sprag_terminal::Unmeasured::Gone => "cgroup is gone",
            };
            let wire = resource_reading(vec![sprag_terminal::PaneResources {
                id: 40,
                taken: sprag_terminal::Taken::Unmeasured { reason },
            }]);

            let answer = render_resources_answer(&wire, &pool(&[40]), &[], &[], None);

            assert!(answer.contains(said), "{reason:?} rendered as: {answer}");
            assert!(
                !answer.contains("0.00 CPU cores"),
                "an unmeasured pane must never read as an idle one: {answer}",
            );
        }
    }

    /// A resolved pane of the caller's own window, for the renderers that take one.
    fn near(number: usize, info: PaneInfo) -> PaneRef {
        PaneRef {
            number: Some(number),
            window: None,
            info,
        }
    }

    fn wire_leaf(pane: u64) -> sprag_terminal::LayoutNodeWire {
        sprag_terminal::LayoutNodeWire::Leaf(PaneId(pane))
    }

    fn wire_split(
        dir: sprag_terminal::SplitDir,
        ratio: f32,
        first: sprag_terminal::LayoutNodeWire,
        second: sprag_terminal::LayoutNodeWire,
    ) -> sprag_terminal::LayoutNodeWire {
        sprag_terminal::LayoutNodeWire::Split {
            id: None,
            dir,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn wire_snapshot(root: sprag_terminal::LayoutNodeWire) -> LayoutSnapshot {
        LayoutSnapshot {
            revision: 5,
            tree: sprag_terminal::LayoutWire { root: Some(root) },
            floating: Vec::new(),
            zoomed: None,
        }
    }

    /// The whole answer, pinned — an agent reads this text, so the thing to assert is the text.
    ///
    /// Three properties at once, none of which survives alone: every pane carries BOTH names (the
    /// number its tools take and the id the user's own CLI prints, which are different integers
    /// here on purpose), the pane this server runs in is marked, and the zoom is named in the words
    /// `zoom-pane` prints.
    #[test]
    fn the_arrangement_is_drawn_in_the_numbers_this_surfaces_tools_take() {
        let mut snapshot = wire_snapshot(wire_split(
            sprag_terminal::SplitDir::Horizontal,
            0.5,
            wire_leaf(40),
            wire_split(
                sprag_terminal::SplitDir::Vertical,
                0.6,
                wire_leaf(41),
                wire_leaf(42),
            ),
        ));
        snapshot.floating = vec![PaneId(43)];
        snapshot.zoomed = Some(PaneId(42));

        assert_eq!(
            render_arrangement_answer(&snapshot, &pool(&[40, 41, 42, 43]), Some(41), None),
            "How YOUR WINDOW's panes are arranged (revision 5):\n\
             \n\
             50% left|right\n\
             ├─ pane 1 (id 40)\n\
             └─ 60% top|bottom\n\
             \x20  ├─ pane 2 (id 41)  (you are here)\n\
             \x20  └─ pane 3 (id 42)  (fills the window)\n\
             floating: pane 4 (id 43)\n\
             \n\
             Which pane is next to which (a direction not listed has no pane that way — that pane \
             is at that edge of the window):\n\
             \x20 pane 1: right=pane 2\n\
             \x20 pane 2: left=pane 1, down=pane 3\n\
             \x20 pane 3: left=pane 1, up=pane 2\n\
             \n\
             Pass a pane NUMBER (not an id) to read_pane, write_pane, send_keys or select_pane. \
             Which pane the user is typing into right now is list_panes' answer, not this one. \
             To MOVE the user beside a pane, do not read a number from here and select it — that \
             is two moments; call select_pane with 'dir' plus 'from' or 'from_here: true' and the \
             terminal resolves it in one.\n",
        );
    }

    /// **The neighbour table is the DAEMON's adjacency, not a reading of the drawing** — asserted
    /// with the control that moves it.
    ///
    /// Two arrangements identical in shape and in every id, differing only in one divider's share.
    /// The pane to the right of pane 1 changes, because adjacency there is settled by which
    /// candidate overlaps it most. Anything derived from the drawing's ORDER — the obvious thing a
    /// re-implementation does — answers the same pane both times and fails the second assertion.
    #[test]
    fn the_neighbour_table_moves_with_the_share_it_is_derived_from() {
        let arrangement = |ratio: f32| {
            wire_snapshot(wire_split(
                sprag_terminal::SplitDir::Horizontal,
                0.5,
                wire_leaf(40),
                wire_split(
                    sprag_terminal::SplitDir::Vertical,
                    ratio,
                    wire_leaf(41),
                    wire_leaf(42),
                ),
            ))
        };
        let neighbours = |ratio: f32| {
            render_arrangement_answer(&arrangement(ratio), &pool(&[40, 41, 42]), None, None)
                .lines()
                .find_map(|line| line.trim().strip_prefix("pane 1: ").map(str::to_owned))
                .expect("pane 1 has a row in the table")
        };

        assert_eq!(neighbours(0.25), "right=pane 3");
        assert_eq!(
            neighbours(0.75),
            "right=pane 2",
            "THE CONTROL: only the share differs, so a table read off the drawing's order would \
             answer the same pane twice",
        );
    }

    /// A leaf whose pane has left the pool between the two reads is REPORTED, not numbered.
    ///
    /// The residual of reading two slots at two instants, and the whole reason the pane list is read
    /// FIRST: numbering this leaf anyway would hand an agent a number that now belongs to a
    /// different pane, and dropping it would make a pane the daemon is still tiling vanish in
    /// silence.
    #[test]
    fn a_pane_that_left_the_pool_between_the_reads_is_named_as_such() {
        let snapshot = wire_snapshot(wire_split(
            sprag_terminal::SplitDir::Horizontal,
            0.5,
            wire_leaf(40),
            wire_leaf(41),
        ));
        let answer = render_arrangement_answer(&snapshot, &pool(&[40]), None, None);
        assert!(
            answer.contains("pane ? (id 41, gone since the pane list was read)"),
            "the leaf with no number says why it has none: {answer}",
        );
        assert!(
            answer.contains("pane 1: right=pane id 41"),
            "and the table names it by the one identity it still has: {answer}",
        );
    }

    /// A window tiling ONE pane has no neighbourhood, so the table is absent rather than empty —
    /// this surface's additive rule, the one `list_panes` follows for a resting pane.
    #[test]
    fn a_single_tiled_pane_gets_no_neighbour_table() {
        let snapshot = wire_snapshot(wire_leaf(40));
        let answer = render_arrangement_answer(&snapshot, &pool(&[40]), Some(40), None);
        assert!(
            answer.contains("pane 1 (id 40)  (you are here)"),
            "the sole pane is still drawn and still marked: {answer}",
        );
        assert!(
            !answer.contains("next to which"),
            "and nothing claims a neighbourhood: {answer}",
        );
    }

    /// A `pane` argument is read as a POSITION or as a NAME by its JSON TYPE, and by nothing else.
    ///
    /// That discrimination is the whole reason the stable handle is a name rather than the host id
    /// this surface already prints: a number and an id are both integers, so one argument could
    /// carry them only behind a mode flag. This test is on the pure parse — resolving either
    /// against a live listing is `resolve_in`'s job, and there is no host here.
    #[test]
    fn a_pane_argument_is_a_position_or_a_name_by_its_json_type() {
        assert_eq!(
            pane_target_at(&json!({ "pane": 2 }), SelectAsk::PANE_KEY).unwrap(),
            PaneTarget::Number(2),
        );
        assert_eq!(
            pane_target_at(&json!({ "pane": "build" }), SelectAsk::PANE_KEY).unwrap(),
            PaneTarget::Name("build".to_owned()),
        );
        assert_eq!(
            pane_target_at(&json!({ "pane": "  build  " }), SelectAsk::PANE_KEY).unwrap(),
            PaneTarget::Name("build".to_owned()),
            "trimmed, so a name resolves the way the daemon stored it",
        );
        // A QUOTED digit string is a name, not a position — which is exactly why the daemon
        // refuses to store an all-digit name: nothing could then be called \"3\", so this can
        // only ever fail to match, never match the wrong pane.
        assert_eq!(
            pane_target_at(&json!({ "pane": "3" }), SelectAsk::PANE_KEY).unwrap(),
            PaneTarget::Name("3".to_owned()),
        );
        assert!(
            pane_target_at(&json!({}), SelectAsk::PANE_KEY).is_err(),
            "a pane must be named"
        );
        assert!(pane_target_at(&json!({ "pane": null }), SelectAsk::PANE_KEY).is_err());
        assert!(pane_target_at(&json!({ "pane": 1.5 }), SelectAsk::PANE_KEY).is_err());
        assert!(pane_target_at(&json!({ "pane": -1 }), SelectAsk::PANE_KEY).is_err());
    }

    /// A name that answers for two panes resolves to NEITHER.
    ///
    /// The daemon holds names unique, so this is unreachable through correct requests — and it is
    /// the residual of the one gap that design leaves (the uniqueness check and the write are not
    /// one atomic step, because making them so would hold the registry lock across a fork). Taking
    /// the first match would rebuild the very failure a name exists to remove.
    #[test]
    fn a_name_two_panes_answer_to_resolves_to_neither() {
        let pane = |id: u64, name: &str| PaneInfo {
            id,
            name: Some(name.to_owned()),
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            shell: None,
            exit_status: None,
            mouse: None,
            focus_tracking: false,
            images: vec![],
            active: false,
            agent: None,
            opened_by: None,
        };
        let panes = vec![pane(10, "build"), pane(11, "build"), pane(12, "test")];
        assert_eq!(pane_by_name(&panes, "test").unwrap().1.id, 12);
        assert_eq!(pane_by_name(&panes, "test").unwrap().0, 2, "its position");
        assert!(
            pane_by_name(&panes, "build").is_err(),
            "two bearers is not one pane"
        );

        // The SESSION-wide resolver is the one a caller actually reaches (a near miss falls
        // through to it), so the sentence that must name the bearers is that one — and it names
        // the WINDOWS too, which is the half a caller cannot otherwise reach. The fixture puts the
        // two bearers in DIFFERENT windows, so a sentence that dropped the window would read as if
        // one pane were listed twice.
        let session: Vec<(String, PaneInfo)> = vec![
            ("0".to_owned(), pane(10, "build")),
            ("docs".to_owned(), pane(11, "build")),
            ("0".to_owned(), pane(12, "test")),
        ];
        assert_eq!(pane_by_name_in_session(&session, "test").unwrap().0, "0");
        let Err(ambiguous) = pane_by_name_in_session(&session, "build") else {
            panic!("two bearers is not one pane");
        };
        assert_eq!(
            ambiguous,
            "more than one pane is called \"build\" (\"build\" (window 0), \"build\" (window \
             docs)), so it does not name one pane. Rename one and try again.",
        );
        // A name nobody carries lists the ones that exist WITH THEIR WINDOWS, so the caller can
        // fix it in one step instead of calling a listing to find out it guessed.
        let Err(missing) = pane_by_name_in_session(&session, "docs") else {
            panic!("no pane is called docs");
        };
        assert_eq!(
            missing,
            "no pane is called \"docs\"; the session's named panes are \"build\" (window 0), \
             \"build\" (window docs), \"test\" (window 0). Call list_windows.",
        );
    }

    #[test]
    fn last_n_lines_keeps_the_trailing_non_blank_lines() {
        let text = "a\n\nb\n\n\nc\nd\n";
        assert_eq!(last_n_lines(text, 2), "c\nd");
        assert_eq!(last_n_lines(text, 100), "a\nb\nc\nd");
        assert_eq!(last_n_lines("", 3), "");
    }

    #[test]
    fn mouse_phrase_maps_wire_tokens_and_passes_unknown_through() {
        assert_eq!(mouse_phrase("click"), "clicks (press/release)");
        assert_eq!(mouse_phrase("button"), "clicks + drag");
        assert_eq!(mouse_phrase("any"), "clicks + drag + motion");
        // A token a future tracking level might add surfaces verbatim, never vanishes.
        assert_eq!(mouse_phrase("future"), "future");
    }

    /// The caveat carries the host's sentence AND the two facts a reader cannot get from it.
    ///
    /// The daemon's message says what is wrong with the file. It does not say what the daemon DID
    /// about it, and that is the part that changes what a reader should do: detection did not stop,
    /// so the verdicts are real readings from a stale rule set, and a pane the file meant to claim
    /// looks unclaimed. A caveat that only forwarded the error would leave `agent_explain` telling a
    /// reader to write a block they have already written.
    #[test]
    fn the_manifest_caveat_says_what_the_daemon_did_not_only_what_broke() {
        let line = manifest_caveat_line("config.toml: no rule `nope` in agent `claude`");
        assert!(
            line.contains("no rule `nope`"),
            "the host's own sentence is carried verbatim: {line}"
        );
        assert!(
            line.contains("last list that worked"),
            "and says detection did not stop, so the verdicts below are real: {line}"
        );
        assert!(
            line.contains("as if no manifest claims it"),
            "and names the reading an unparsed claim produces, which is the trap: {line}"
        );
    }

    #[test]
    fn parse_pane_info_reads_mouse_and_focus_from_the_wire_entry() {
        // A tracking pane: the additive `mouse` / `focus_tracking` keys are present.
        let info = parse_pane_info(&json!({
            "id": 5, "cols": 80, "rows": 24, "command": "htop", "title": null,
            "mouse": "any", "focus_tracking": true
        }));
        assert_eq!(info.id, 5);
        assert_eq!(info.mouse.as_deref(), Some("any"));
        assert!(info.focus_tracking);
        // A resting pane: neither key present -> the resting defaults (None / false), never a panic.
        let resting = parse_pane_info(&json!({ "id": 1, "command": "bash", "title": null }));
        assert_eq!(resting.mouse, None);
        assert!(!resting.focus_tracking);
    }

    /// A pane in the batch is named in BOTH vocabularies, and one that is no longer in the list
    /// says so rather than being numbered.
    ///
    /// The residual is the half no live test can drive on demand — a pane closing between the
    /// events read and the pane read — and it is the half that matters: numbering a pane that is
    /// gone would hand the caller a number that now belongs to a DIFFERENT pane. `pane_closed`
    /// reaches it on every occurrence, which is correct and is asserted here so the wording is a
    /// decision rather than an accident.
    ///
    /// A window subject is carried through unchanged, as the control that this joins PANES and not
    /// every subject it meets.
    #[test]
    fn a_change_names_its_pane_in_both_vocabularies_or_says_it_is_gone() {
        let live = PaneInfo {
            id: 11,
            name: None,
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            opened_by: None,
            active: false,
            shell: None,
            exit_status: None,
            mouse: None,
            focus_tracking: false,
            images: vec![],
            agent: None,
        };
        // Three rows so the NUMBER and the ID cannot be confused (id 11 is the third pane), a
        // pane one window over, and a pane in none — the three answers this rendering has to tell
        // apart, in one fixture where they all disagree.
        let here = vec![
            PaneInfo {
                id: 40,
                ..live.clone()
            },
            PaneInfo {
                id: 41,
                ..live.clone()
            },
            live.clone(),
        ];
        let session = vec![(
            "build".to_owned(),
            PaneInfo {
                id: 7,
                ..live.clone()
            },
        )];
        let events = vec![
            json!({ "type": "pane_job_changed", "pane": 11 }),
            json!({ "type": "pane_job_changed", "pane": 7 }),
            json!({ "type": "pane_closed", "pane": 4 }),
            json!({ "type": "window_selected", "window": "build" }),
        ];
        assert_eq!(
            render_events(&events, &here, &session),
            "  pane_job_changed: pane 3 (id 11)\n  pane_job_changed: pane id 7 (window build)\n  \
             pane_closed: pane ? (id 4, gone since the pane list was read)\n  window_selected: \
             window build\n",
        );
    }

    /// The provenance line names its opener THREE ways, and only two of them are reachable live.
    ///
    /// "you" and "pane N" are pinned end to end by `mcp_stdio`. The third — an opener this window
    /// does not hold — is not: it needs a pane in ANOTHER window to have opened one in this one, and
    /// building that live would test the harness rather than the rendering. Pinned here instead of
    /// registered as a gap, because the sentence is the whole point: a number means nothing outside
    /// the listing that indexes it, so an absent opener must NOT be rendered as one.
    #[test]
    fn the_provenance_line_names_an_opener_this_window_does_not_hold_by_id() {
        let opened = |opener: u64| PaneInfo {
            id: 7,
            name: None,
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            opened_by: Some(opener),
            shell: None,
            exit_status: None,
            mouse: None,
            focus_tracking: false,
            images: Vec::new(),
            active: false,
            agent: None,
        };
        // The listing this rendering indexes into: pane 1 is host id 3, the opener.
        let listing = [PaneInfo {
            id: 3,
            name: None,
            ..opened(0)
        }];
        assert!(
            pane_summary(1, &opened(3), &listing, None).contains("      opened by: pane 1\n"),
            "an opener this window holds is named by its NUMBER",
        );
        assert!(
            pane_summary(1, &opened(99), &listing, None)
                .contains("      opened by: pane id 99, not in this window\n"),
            "and one it does not hold is named by the id that still addresses it, with the reason \
             it has no number here — never by a number this listing would make up",
        );
        assert!(
            pane_summary(1, &opened(3), &listing, Some(3)).contains(
                "      opened by: you (yours to \
             close)\n"
            ),
            "and the caller's own panes say so, which is the only value close_pane accepts",
        );
    }

    #[test]
    fn pane_summary_surfaces_mouse_and_focus_tracking() {
        let tracking = PaneInfo {
            id: 7,
            name: None,
            title: String::new(),
            command: "vim".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            opened_by: None,
            active: false,
            shell: None,
            exit_status: None,
            mouse: Some("any".to_owned()),
            focus_tracking: true,
            images: vec![],
            agent: None,
        };
        let summary = pane_summary(1, &tracking, &[], None);
        assert!(
            summary.contains("mouse: tracking clicks + drag + motion"),
            "the mouse-tracking level must surface: {summary}"
        );
        assert!(
            summary.contains("focus: tracking focus in/out"),
            "the focus-tracking mode must surface: {summary}"
        );
        // A resting pane (child tracking neither) emits NEITHER line — the additive default, so the
        // header stays uncluttered for the common case.
        let resting = PaneInfo {
            mouse: None,
            focus_tracking: false,
            ..tracking
        };
        let resting = pane_summary(1, &resting, &[], None);
        assert!(
            !resting.contains("mouse:"),
            "no mouse line when off: {resting}"
        );
        assert!(
            !resting.contains("focus:"),
            "no focus line when off: {resting}"
        );
    }

    /// The additive `agent` object off the wire, and the three shapes that must NOT become a state:
    /// a missing key, a `null`, and an object with no `state`.
    ///
    /// A defaulted state is the specific failure worth a test here: `idle` means "an agent is waiting
    /// for you", so inventing one for a pane running a shell would send a sibling agent to interrupt a
    /// human who was never asked for anything.
    ///
    /// REVERT-PROOF: give `state` a fallback (`unwrap_or("idle")`) and TWO of the three rejections
    /// fail — the `null` and the state-less object. The missing-key case survives it, because
    /// `entry.get("agent")?` has already short-circuited; measured rather than assumed, and worth
    /// naming because that third case is the only one a real daemon produces (it omits the key for a
    /// pane no manifest claims), which is why `tests/mcp_stdio.rs` cannot hold this guard at all.
    #[test]
    fn parse_agent_info_reads_a_verdict_and_never_invents_one() {
        let claimed = parse_agent_info(&json!({
            "id": 1,
            "agent": { "state": "blocked", "name": "claude", "rule": "dialog-choice-list", "seq": 4 }
        }))
        .expect("a well-formed verdict parses");
        assert_eq!(claimed.state, "blocked");
        assert_eq!(claimed.name.as_deref(), Some("claude"));
        assert_eq!(claimed.rule.as_deref(), Some("dialog-choice-list"));
        assert_eq!(claimed.seq, 4);

        // A pane no manifest claims — the ordinary shell, and the whole population this surface must
        // not describe as an agent at rest.
        assert!(parse_agent_info(&json!({ "id": 1, "command": "bash" })).is_none());
        assert!(parse_agent_info(&json!({ "id": 1, "agent": null })).is_none());
        assert!(
            parse_agent_info(&json!({ "id": 1, "agent": { "seq": 2 } })).is_none(),
            "an object with no state is not a state",
        );
        // `name` and `rule` are optional ON THE WIRE (R251: a modal can cover the fingerprint), so a
        // verdict without them is still a verdict.
        let nameless = parse_agent_info(&json!({ "id": 1, "agent": { "state": "working" } }))
            .expect("a state alone is enough");
        assert_eq!(
            (nameless.name, nameless.rule, nameless.seq),
            (None, None, 0)
        );
    }

    /// The pane list's agent line, and the additive rule that keeps it off every other pane.
    ///
    /// The state TOKEN is carried rather than glossed, unlike the human surfaces: this line's reader
    /// is a program that will branch on the value, and `agent_explain` is where the prose lives.
    ///
    /// REVERT-PROOF: make the line unconditional and the shell assertion fails — every pane in the
    /// terminal would claim an agent state, which is D3's one forbidden collapse.
    #[test]
    fn pane_summary_surfaces_a_sibling_agents_state_and_omits_it_for_a_shell() {
        let shell = PaneInfo {
            id: 3,
            name: None,
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            opened_by: None,
            active: false,
            shell: None,
            exit_status: None,
            mouse: None,
            focus_tracking: false,
            images: vec![],
            agent: None,
        };
        let claimed = PaneInfo {
            agent: Some(AgentInfo {
                state: "blocked".to_owned(),
                name: Some("claude".to_owned()),
                rule: Some("dialog-choice-list".to_owned()),
                source: None,
                seq: 4,
            }),
            ..shell
        };
        let summary = pane_summary(1, &claimed, &[], None);
        assert!(
            summary.contains("agent: state=blocked name=claude rule=dialog-choice-list seq=4"),
            "the verdict surfaces field for field: {summary}",
        );

        // A REPORTED verdict carries who said so and no rule. An agent reading this line acts on
        // the difference: a scraped verdict is corrected by editing a manifest, a reported one only
        // by releasing the pane, so a surface that showed neither would be advising a guess.
        let reported = PaneInfo {
            agent: Some(AgentInfo {
                state: "working".to_owned(),
                name: Some("claude".to_owned()),
                rule: None,
                source: Some("hook:claude".to_owned()),
                seq: 5,
            }),
            ..claimed
        };
        let summary = pane_summary(1, &reported, &[], None);
        assert!(
            summary.contains("agent: state=working name=claude source=hook:claude seq=5"),
            "an authority is told from an inference: {summary}",
        );
        let quiet = pane_summary(
            1,
            &PaneInfo {
                agent: None,
                ..reported
            },
            &[],
            None,
        );
        assert!(
            !quiet.contains("agent:"),
            "a pane no manifest claims says nothing about an agent: {quiet}",
        );
    }

    #[test]
    fn env_from_bytes_parses_nul_separated_records() {
        let buf = b"A=1\0SPRAG_HOST_RPC_SOCK=/run/user/1000/x.sock\0B=2\0";
        assert_eq!(
            env_from_bytes(buf, "SPRAG_HOST_RPC_SOCK").as_deref(),
            Some("/run/user/1000/x.sock")
        );
        assert_eq!(env_from_bytes(buf, "A").as_deref(), Some("1"));
        assert_eq!(env_from_bytes(buf, "MISSING"), None);
        // A key that is a prefix of another key must not false-match.
        assert_eq!(env_from_bytes(b"AB=x\0", "A"), None);
    }

    #[test]
    fn read_proc_env_reads_an_inherited_var_from_real_proc() {
        // `/proc/<pid>/environ` reflects the environment at exec (NOT runtime
        // set_var), which is exactly the ancestor case this function serves. PATH is
        // in the test binary's exec environment, so it is present; a made-up var is not.
        let me = std::process::id();
        assert!(read_proc_env(me, "PATH").is_some());
        assert_eq!(read_proc_env(me, "SPRAG_MCP_DEFINITELY_ABSENT_VAR"), None);
    }
}
