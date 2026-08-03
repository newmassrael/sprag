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
//! JSON-RPC 2.0 on stdin/stdout. It advertises self-describing tools —
//! `list_panes`, `pane_layout`, `pane_processes`, `read_pane`, `read_last_command`,
//! `read_pane_links`, `read_pane_images`, `find_in_pane`, `regex_in_pane`, `agent_state`,
//! `agent_explain`, `wait_for_change`, `write_pane`, `send_keys`, `select_pane` — so an agent *immediately*
//! understands "read/write a sibling pane" without reading any sprag source. (Named rather than
//! counted, for the reason [`tools_list`] gives: a count kept in prose goes stale silently, and this
//! one had.) The two `agent_*` tools are the surface for the one fact an agent cannot read off a
//! sibling's screen without interpreting it: whether the AI in that pane is waiting for a human
//! (H3). They report the daemon's own verdict, so two agents watching one pane agree.
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
//! working before typing somewhere else.
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
use sprag_host::shellword::shell_quote;
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, EVENTS_WAIT_METHOD, FULL_TEXT_SLOT, KEY_ACTION, LAST_COMMAND_SLOT,
    LAYOUT_SLOT, LINKS_SLOT, PANES_SLOT, PaneProcessesWire, SELECT_PANE_ACTION, SINCE_PARAM,
    TEXT_ACTION, find_slot_for, pane_processes_at, regex_slot_for,
};
use sprag_host::{PANE_ENV_VAR, PaneFind, mux_action_path, pane_input_path};
use sprag_rpc::{CallError, HostConn, INVALID_PARAMS};
use sprag_terminal::{LayoutSnapshot, PaneDir, PaneId, arrangement};

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
        "instructions": "You are running inside a pane of a sprag terminal. These \
            tools let you observe and drive the OTHER (sibling) panes as data: read a \
            pane's on-screen text and scrollback, read just a pane's last command and \
            its result, ask whether the AI AGENT in a sibling pane is working, waiting for \
            a human, or at rest (`agent_state`, and `agent_explain` for why), type text \
            into a pane, or send keys. \
            Call `list_panes` first to see the pane numbers (1 = first pane). \
            \"pane 2\" means the second pane in that list. \
            Call `pane_layout` when position matters — it draws how the panes are \
            arranged, marks the pane YOU are in, and says which pane is to the left, \
            right, above and below each one, so \"the pane next to mine\" resolves to a \
            number you can pass to the other tools. If a tool reports it is not \
            inside a sprag terminal, these tools do not apply to this session."
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
    let pane_arg = json!({
        "type": "integer",
        "minimum": 1,
        "description": "1-based pane number as shown by list_panes (1 = the first pane)."
    });
    json!({
        "tools": [
            {
                "name": "list_panes",
                "description": "List the sibling terminal panes in this sprag window, \
                    with their 1-based number, host id, size, running command, live \
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
                "name": "pane_layout",
                "description": "Draw WHERE the panes sit — the window's arrangement as a tree of \
                    divisions, which pane (if any) is zoomed to fill the window, which panes are \
                    floated out of the tiling, and WHICH PANE IS NEXT TO WHICH in each direction. \
                    `list_panes` answers WHO is in this terminal; this answers WHERE, so it is what \
                    to call before choosing a pane by position (\"the pane to the right of mine\", \
                    \"the one below\"). It also marks the pane YOU are running in, which is what \
                    makes a direction mean anything. The neighbour table is the daemon's own \
                    adjacency — the same answer the user's own directional keybinding moves by — \
                    so you never have to work it out from the shape. Which pane the USER is \
                    currently typing into is NOT here: that changes on a keystroke and belongs to \
                    `list_panes`, which marks it.",
                "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
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
                    `pane_created`, `pane_closed`, `pane_selected`, `window_created`, \
                    `window_closed`, `window_selected`, `session_created`, `session_closed`, \
                    `layout_updated` — each naming its SUBJECT, not its new value: follow up with \
                    agent_state, pane_processes or list_panes to read the subject it names. To \
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
                            "type": "integer",
                            "minimum": 1,
                            "description": "Only wake for changes about THIS pane (1 = the first \
                                pane in list_panes). Omit to hear about every pane."
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
                "name": "select_pane",
                "description": "Make a pane the ACTIVE one for this terminal session — where \
                    the user's keystrokes go, what every attached window shows a focus ring \
                    on, and what a pane command with no target acts on. Use it to put a human \
                    on the pane you want them to look at, after opening or preparing it. \
                    `list_panes` marks the active pane. This MOVES a person's cursor: prefer \
                    it when you have something for them to see, not as a side effect.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "pane": pane_arg },
                    "required": ["pane"],
                    "additionalProperties": false
                }
            }
        ]
    })
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
        "pane_layout" => tool_pane_layout(),
        "pane_processes" => tool_pane_processes(&args),
        "read_pane" => tool_read_pane(&args),
        "read_last_command" => tool_read_last_command(&args),
        "read_pane_links" => tool_read_pane_links(&args),
        "read_pane_images" => tool_read_pane_images(&args),
        "find_in_pane" => tool_find_in_pane(&args),
        "regex_in_pane" => tool_regex_in_pane(&args),
        "agent_state" => tool_agent_state(&args),
        "agent_explain" => tool_agent_explain(&args),
        "wait_for_change" => tool_wait_for_change(&args),
        "write_pane" => tool_write_pane(&args),
        "send_keys" => tool_send_keys(&args),
        "select_pane" => tool_select_pane(&args),
        other => Err(format!("unknown tool: {other}")),
    };
    match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": format!("Error: {error}") }],
            "isError": true
        }),
    }
}

// ---- Tools ---------------------------------------------------------------------

/// One pane as the host's pane-list reports it, plus its display number.
struct PaneInfo {
    number: usize,
    id: u64,
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
}

/// One pane's agent verdict as an agent reads it — the wire's own `agent` object, field for field.
///
/// The state token is carried rather than glossed. A human surface renders `blocked` as a phrase
/// (`sprag_client::agent_phrase`, shared by both frontends so they cannot drift); this surface's reader
/// is a program that will branch on the value, so it gets the vocabulary and the tool description
/// teaches what each token means.
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
    let panes = query_panes()?;
    if panes.is_empty() {
        return Ok("This sprag terminal has no panes.".to_owned());
    }
    let mut out = format!("{} pane(s) in this sprag terminal:\n", panes.len());
    for pane in &panes {
        out.push_str(&pane_summary(pane));
    }
    Ok(out)
}

/// Render ONE pane as its `list_panes` block — the header line plus an indented line per live
/// signal the pane raised. Each sub-line is emitted ONLY when its signal is present, so a resting
/// pane is just the header (mirrors the additive wire). Split out as a pure function so the
/// invisible-state lines (mouse / focus) are unit-testable without a live host.
fn pane_summary(pane: &PaneInfo) -> String {
    let title = if pane.title.is_empty() {
        "(none)".to_owned()
    } else {
        format!("{:?}", pane.title)
    };
    // The ACTIVE marker rides the header line rather than an indented one: it is a property OF the
    // pane's identity in the window, like its number, not a signal the pane raised.
    let active = if pane.active { " (active)" } else { "" };
    let mut out = format!(
        "  pane {}: id={} {}x{} command={} title={}{active}\n",
        pane.number, pane.id, pane.cols, pane.rows, pane.command, title
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
fn tool_pane_layout() -> Result<String, String> {
    let panes = query_panes()?;
    let answer = host_call(
        "scene/query",
        json!({ "path": mux_action_path(LAYOUT_SLOT) }),
    )?;
    // Through the SSOT type, never by walking the served arena by hand: it is a flat arena whose
    // nodes name their children by index, and a second reader of that encoding is a second thing
    // that can come to disagree with the daemon about what it means.
    let snapshot: LayoutSnapshot = serde_json::from_value(answer)
        .map_err(|error| format!("the host's arrangement did not parse: {error}"))?;
    Ok(render_arrangement_answer(&snapshot, &panes, own_pane()))
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
    let wanted = match args.get("pane") {
        None | Some(Value::Null) => None,
        Some(_) => Some(pane_number(args)?),
    };
    let panes = query_panes()?;
    let answer = host_call(
        "scene/query",
        json!({ "path": mux_action_path(&pane_processes_at(0)) }),
    )?;
    // Through the SSOT type: a second reader of the served shape is a second thing that can come to
    // disagree with the daemon about what a field means.
    let wire: PaneProcessesWire = serde_json::from_value(answer)
        .map_err(|error| format!("the host's process reading did not parse: {error}"))?;
    if let Some(number) = wanted
        && !panes.iter().any(|pane| pane.number == number)
    {
        return Err(format!(
            "no pane {number} (this terminal has {} pane(s); call list_panes)",
            panes.len()
        ));
    }
    Ok(render_processes_answer(&wire, &panes, wanted))
}

/// The text [`tool_pane_processes`] returns, as a pure function of what was read — so every shape is
/// testable without a live host, and the integration test can pin what an agent actually receives.
///
/// `wanted` is the 1-based pane number the caller asked about, or `None` for all of them.
fn render_processes_answer(
    wire: &PaneProcessesWire,
    panes: &[PaneInfo],
    wanted: Option<usize>,
) -> String {
    let number_of = |id: u64| panes.iter().find(|p| p.id == id).map(|p| p.number);
    let rows: Vec<_> = wire
        .panes
        .iter()
        .filter(|row| match wanted {
            // A row whose pane the list no longer holds cannot match a NUMBER, so a narrowed answer
            // simply does not include it; the unnarrowed answer says it is there and unnameable.
            Some(number) => number_of(row.id) == Some(number),
            None => true,
        })
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
        let name = number_of(row.id).map_or_else(
            // The residual of the two reads, said rather than smoothed over.
            || format!("pane ? (id {}, gone since the pane list was read)", row.id),
            |number| format!("pane {number} (id {})", row.id),
        );
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
) -> String {
    let number_of = |pane: PaneId| panes.iter().find(|p| p.id == pane.0).map(|p| p.number);
    // The DRAWING's naming. Both integers, always: the number is what this surface's tools take, and
    // the id is what the same arrangement is called by `sprag layout`, the daemon's logs and the
    // user's own CLI — so an agent reporting to a human, and a human checking the agent, are not
    // holding two pictures that share no name.
    let label = |pane: PaneId| {
        let Some(number) = number_of(pane) else {
            // The residual of the two reads, said rather than smoothed over. Numbering it anyway
            // would hand the caller a number that now belongs to a DIFFERENT pane.
            return format!("pane ? (id {pane}, gone since the pane list was read)");
        };
        let mine = if here == Some(pane.0) {
            "  (you are here)"
        } else {
            ""
        };
        format!("pane {number} (id {pane}){mine}")
    };

    let mut out = format!(
        "How this sprag terminal's panes are arranged (revision {}):\n\n",
        snapshot.revision
    );
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
         Which pane the user is typing into right now is list_panes' answer, not this one.\n",
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
    let id = resolve_pane_id(args)?;
    let value = host_call(
        "scene/query",
        json!({ "path": pane_input_path(id, FULL_TEXT_SLOT) }),
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
    let id = resolve_pane_id(args)?;
    let value = host_call(
        "scene/query",
        json!({ "path": pane_input_path(id, LAST_COMMAND_SLOT) }),
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
    let id = resolve_pane_id(args)?;
    let value = host_call(
        "scene/query",
        json!({ "path": pane_input_path(id, LINKS_SLOT) }),
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
    let number = pane_number(args)?;
    let panes = query_panes()?;
    let pane = panes
        .get(number - 1)
        .ok_or_else(|| format!("no pane number {number} (there are {})", panes.len()))?;
    if pane.images.is_empty() {
        return Ok("This pane shows no inline images.".to_owned());
    }
    let mut out = format!("{} image(s) in pane {number}:\n", pane.images.len());
    for img in &pane.images {
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
    let id = resolve_pane_id(args)?;
    let needle = args
        .get("needle")
        .and_then(Value::as_str)
        .filter(|needle| !needle.is_empty())
        .ok_or("find_in_pane needs a non-empty `needle`")?;
    search_pane(id, &find_slot_for(needle), needle)
}

/// `regex_in_pane` — the same search read as a REGULAR EXPRESSION.
///
/// A separate tool rather than a flag on `find_in_pane`, all the way up from the wire: a needle and
/// a pattern are separate languages in which the same string means different things, so which one
/// an agent means is expressed by WHICH TOOL it calls, not by an argument that could be defaulted,
/// forgotten, or carried over from a previous call.
fn tool_regex_in_pane(args: &Value) -> Result<String, String> {
    let id = resolve_pane_id(args)?;
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .filter(|pattern| !pattern.is_empty())
        .ok_or("regex_in_pane needs a non-empty `pattern`")?;
    search_pane(id, &regex_slot_for(pattern), pattern)
}

/// Query pane `id` at `slot` and render the matching lines as `LINE: text`.
///
/// The ONE renderer both search tools use, so a literal hit and a regex hit read identically to an
/// agent — only the language of `wanted` (echoed in the no-match message) differs. Neither tool
/// implements a search: both read a host query, so they agree with the CLI and the GUI highlight.
fn search_pane(id: u64, slot: &str, wanted: &str) -> Result<String, String> {
    let value = host_call("scene/query", json!({ "path": pane_input_path(id, slot) }))?;
    let found: PaneFind =
        serde_json::from_value(value).map_err(|error| format!("malformed find answer: {error}"))?;
    // A refused pattern is an ERROR, not an empty result: "your pattern is wrong" and "nothing
    // matched" are different answers, and an agent that cannot tell them apart will retry forever.
    if let Some(error) = found.error {
        return Err(format!("invalid pattern {wanted:?}: {error}"));
    }
    if found.lines.is_empty() {
        return Ok(format!("no matches for {wanted:?} in pane {id}"));
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

fn tool_write_pane(args: &Value) -> Result<String, String> {
    let number = pane_number(args)?;
    let id = pane_id_for(number)?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or("missing required string argument 'text'")?;
    host_call(
        "scene/invoke",
        json!({ "path": pane_input_path(id, TEXT_ACTION), "args": { "text": text } }),
    )?;
    let enter = args.get("enter").and_then(Value::as_bool).unwrap_or(true);
    if enter {
        host_call(
            "scene/invoke",
            json!({ "path": pane_input_path(id, KEY_ACTION), "args": { "key": "Enter" } }),
        )?;
    }
    Ok(format!(
        "Wrote {} byte(s) to pane {number}{}.",
        text.len(),
        if enter { " and pressed Enter" } else { "" }
    ))
}

/// `select_pane` — move the SESSION's active pane, which every attached client follows.
///
/// The answer names what actually happened rather than echoing the request: the daemon reports
/// whether the pane MOVED, and a re-select of the pane the session is already on is a legitimate
/// no-op an agent should not read as a failure.
fn tool_select_pane(args: &Value) -> Result<String, String> {
    let number = pane_number(args)?;
    let id = pane_id_for(number)?;
    let answer = host_call(
        "scene/invoke",
        json!({ "path": mux_action_path(SELECT_PANE_ACTION), "args": { "pane": id } }),
    )?;
    if answer["changed"] == json!(true) {
        Ok(format!(
            "Pane {number} is now the active pane of this session."
        ))
    } else {
        Ok(format!(
            "Pane {number} was already the active pane; nothing moved."
        ))
    }
}

fn tool_send_keys(args: &Value) -> Result<String, String> {
    let number = pane_number(args)?;
    let id = pane_id_for(number)?;
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
            json!({
                "path": pane_input_path(id, KEY_ACTION),
                "args": { "key": key, "ctrl": ctrl, "alt": alt, "shift": shift, "super": sup }
            }),
        )?;
    }
    Ok(format!("Sent {} key(s) to pane {number}.", keys.len()))
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
    let panes = query_panes()?;
    if panes.is_empty() {
        return Ok("This sprag terminal has no panes.".to_owned());
    }
    let selected: Vec<&PaneInfo> = match args.get("pane") {
        Some(_) => {
            let number = pane_number(args)?;
            vec![panes.iter().find(|p| p.number == number).ok_or_else(|| {
                format!(
                    "no pane {number}; this terminal has {} pane(s). Call list_panes.",
                    panes.len()
                )
            })?]
        }
        None => panes.iter().collect(),
    };
    let mut out = String::new();
    for pane in selected {
        match &pane.agent {
            Some(agent) => out.push_str(&format!(
                "  pane {}: id={} {}\n",
                pane.number,
                pane.id,
                agent_line(agent)
            )),
            None => out.push_str(&format!(
                "  pane {}: id={} no agent (no manifest claims this pane — not the same as idle)\n",
                pane.number, pane.id
            )),
        }
    }
    Ok(out)
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
/// **22 431 returns a second** (build-rate pane, every answer empty) against **zero** for a quiet one.
/// For an agent that meant *"wait until the build in pane 2 finishes"* — the use case this tool's own
/// description names — could not be expressed at all, and each useless return cost a tool result and
/// an LLM turn.
///
/// `events/waitFor {since, match}` parks on the JOURNAL instead, and the filter is applied by the
/// daemon under the lock that appends. Output is not a record, so it cannot wake this; another pane's
/// change does not either, when the caller named its own.
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
    /// Where this server has read up to. `None` until the first call, which starts at the present:
    /// replaying a daemon's whole history to a caller asking "what happens next" would bury the
    /// answer under a backlog it did not ask for.
    static CURSOR: Mutex<Option<u64>> = Mutex::new(None);

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
        None => host_call("scene/revision", json!({}))?["revision"]
            .as_u64()
            .ok_or("the host did not report a scene revision")?,
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
        params["match"] = filter;
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
    // 1-based NUMBER. So the ids are joined against the pane list before they are printed — over
    // the SAME connection, for the reason the park and the batch already share one.
    //
    // **The comment below used to claim this and the code did not do it**, which made the answer
    // name a pane in a vocabulary no other tool here accepts: an agent told `pane id=0` cannot pass
    // `0` to `pane_processes`, whose pane numbers start at 1. Found by reading what the tool
    // actually printed.
    //
    // Read only when a pane subject is present: a window or session change pays nothing for it.
    let panes: Vec<PaneInfo> = if events.iter().any(|event| event["pane"].is_u64()) {
        conn.call(
            "scene/query",
            json!({ "path": mux_action_path(PANES_SLOT) }),
        )
        .map_err(|e| e.to_string())?
        .as_array()
        .ok_or("the host pane list was not an array")?
        .iter()
        .enumerate()
        .map(|(index, pane)| parse_pane_info(index, pane))
        .collect()
    } else {
        Vec::new()
    };

    out.push_str(&render_events(events, &panes));
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
        Some(_) => Some(resolve_pane_id(args)?),
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
fn render_events(events: &[Value], panes: &[PaneInfo]) -> String {
    let mut out = String::new();
    for event in events {
        let kind = event["type"].as_str().unwrap_or("?");
        match (
            event["pane"].as_u64(),
            event["window"].as_str(),
            event["session"].as_str(),
        ) {
            (Some(id), _, _) => {
                // Both integers travel: the number is what this surface's tools take, and the id is
                // what `sprag panes`, the daemon's logs and the user's own CLI call the same pane,
                // so an agent reporting to a human and a human checking the agent hold one picture.
                match panes
                    .iter()
                    .find(|pane| pane.id == id)
                    .map(|pane| pane.number)
                {
                    Some(number) => out.push_str(&format!("  {kind}: pane {number} (id {id})\n")),
                    // The residual of the two reads, said rather than smoothed over — and
                    // `pane_closed` is the one event that reaches it every time, correctly:
                    // numbering a pane that is gone would hand the caller a number that now belongs
                    // to a DIFFERENT pane.
                    None => out.push_str(&format!(
                        "  {kind}: pane ? (id {id}, gone since the pane list was read)\n"
                    )),
                }
            }
            (_, Some(name), _) => out.push_str(&format!("  {kind}: window {name}\n")),
            (_, _, Some(name)) => out.push_str(&format!("  {kind}: session {name}\n")),
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
    let number = pane_number(args)?;
    let panes = query_panes()?;
    let pane = panes.iter().find(|p| p.number == number).ok_or_else(|| {
        format!(
            "no pane {number}; this terminal has {} pane(s). Call list_panes.",
            panes.len()
        )
    })?;
    // In front of EVERY branch below, and most of all the one that says no manifest claims this
    // pane: that sentence is also what an unparsed claim looks like from here, and sending a reader
    // off to write an `[[agent]]` block they have already written is the trap this closes.
    let mut out = manifest_caveat().unwrap_or_default();
    let Some(agent) = &pane.agent else {
        out.push_str(&format!(
            "pane {number} (id={}) has no agent state: no agent manifest claims this pane, so no \
             rule was even consulted for it. That is what an ordinary shell looks like. If this pane \
             IS running an agent sprag does not know, add an `[[agent]]` block to sprag's config.toml \
             with a fingerprint that matches its screen or title.\n",
            pane.id
        ));
        return Ok(out);
    };
    out.push_str(&format!(
        "pane {number} (id={}) is {}",
        pane.id, agent.state
    ));
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

/// The requested 1-based pane number from a tool's arguments.
fn pane_number(args: &Value) -> Result<usize, String> {
    let n = args
        .get("pane")
        .and_then(Value::as_u64)
        .ok_or("missing required integer argument 'pane' (1-based, see list_panes)")?;
    usize::try_from(n).map_err(|_| "pane number out of range".to_owned())
}

/// Resolve a tool's `pane` argument to a host pane id (one list query).
fn resolve_pane_id(args: &Value) -> Result<u64, String> {
    pane_id_for(pane_number(args)?)
}

/// Map a 1-based pane number to its host id against the live pane list.
fn pane_id_for(number: usize) -> Result<u64, String> {
    let panes = query_panes()?;
    panes
        .iter()
        .find(|p| p.number == number)
        .map(|p| p.id)
        .ok_or_else(|| {
            format!(
                "no pane {number}; this terminal has {} pane(s). Call list_panes.",
                panes.len()
            )
        })
}

/// Query the host's live pane list, numbered 1-based in host order.
fn query_panes() -> Result<Vec<PaneInfo>, String> {
    let value = host_call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    )?;
    let array = value
        .as_array()
        .ok_or("the host pane list was not an array")?;
    Ok(array
        .iter()
        .enumerate()
        .map(|(index, pane)| parse_pane_info(index, pane))
        .collect())
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

/// Parse one panes-slot entry into a [`PaneInfo`], numbered 1-based from its host-order `index`.
/// Every field is ADDITIVE on the wire (present only when its signal fired), so a missing key maps
/// to the resting default — split out as a pure function so the parse is testable without a live
/// host (mirrors [`parse_image_info`]).
fn parse_pane_info(index: usize, pane: &Value) -> PaneInfo {
    PaneInfo {
        number: index + 1,
        id: pane.get("id").and_then(Value::as_u64).unwrap_or(0),
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
fn host_call(method: &str, params: Value) -> Result<Value, String> {
    let sock = host_sock().ok_or_else(|| {
        "not inside a sprag terminal (no SPRAG_HOST_RPC_SOCK in this process or any \
         ancestor); these pane tools do not apply to this session"
            .to_owned()
    })?;
    let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)
        .map_err(|e| format!("cannot reach the sprag host at {}: {e}", sock.display()))?;
    conn.call(method, params).map_err(|e| e.to_string())
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

/// Read the parent PID of `pid` from `/proc/<pid>/status` (`PPid:` line).
fn read_ppid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|l| l.strip_prefix("PPid:"))
        .and_then(|rest| rest.trim().parse().ok())
}

/// Read `key`'s value from `/proc/<pid>/environ` (NUL-separated `KEY=VALUE` records).
fn read_proc_env(pid: u32, key: &str) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    env_from_bytes(&bytes, key)
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

    #[test]
    fn tools_list_advertises_every_tool_with_object_schemas() {
        let tools = tools_list();
        let names: Vec<&str> = tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_panes",
                "pane_layout",
                "pane_processes",
                "read_pane",
                "read_last_command",
                "read_pane_links",
                "read_pane_images",
                "find_in_pane",
                "regex_in_pane",
                "agent_state",
                "wait_for_change",
                "agent_explain",
                "write_pane",
                "send_keys",
                "select_pane"
            ]
        );
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
        // ...and `agent_state` requires NOTHING, which is the one asymmetry in this roster and is
        // deliberate: "which pane needs a human" is a question about the SET, so the whole-terminal
        // form is the one an agent asks first. A `required: ["pane"]` here would force it to ask
        // once per pane and assemble the answer itself.
        assert_eq!(required("agent_state"), json!(null));
        // `pane_layout` takes nothing either, and for its own reason: the arrangement is one thing
        // and the whole of it is the answer. A `pane` argument would make a caller ask per pane and
        // reassemble a shape the daemon already published in one piece.
        assert_eq!(required("pane_layout"), json!(null));
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
        assert!(result["instructions"].as_str().unwrap().contains("sibling"));
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
            render_processes_answer(&wire, &pool(&[40, 41]), None),
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
        let answer = render_processes_answer(&wire, &pool(&[40, 41]), None);

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
        let answer = render_processes_answer(&wire, &pool(&[40, 41]), Some(2));
        assert!(answer.contains("pane 2 (id 41)"), "{answer}");
        assert!(!answer.contains("pane 1 (id 40)"), "{answer}");
    }

    /// A pane list of `n` panes whose host ids are deliberately NOT their numbers — the mapping this
    /// surface exists to keep straight, and one an off-by-anything would pass with ids of `1..=n`.
    fn pool(ids: &[u64]) -> Vec<PaneInfo> {
        ids.iter()
            .enumerate()
            .map(|(index, id)| parse_pane_info(index, &json!({ "id": id })))
            .collect()
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
            render_arrangement_answer(&snapshot, &pool(&[40, 41, 42, 43]), Some(41)),
            "How this sprag terminal's panes are arranged (revision 5):\n\
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
             Which pane the user is typing into right now is list_panes' answer, not this one.\n",
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
            render_arrangement_answer(&arrangement(ratio), &pool(&[40, 41, 42]), None)
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
        let answer = render_arrangement_answer(&snapshot, &pool(&[40]), None);
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
        let answer = render_arrangement_answer(&snapshot, &pool(&[40]), Some(40));
        assert!(
            answer.contains("pane 1 (id 40)  (you are here)"),
            "the sole pane is still drawn and still marked: {answer}",
        );
        assert!(
            !answer.contains("next to which"),
            "and nothing claims a neighbourhood: {answer}",
        );
    }

    #[test]
    fn pane_number_requires_a_positive_integer() {
        assert_eq!(pane_number(&json!({ "pane": 2 })).unwrap(), 2);
        assert!(pane_number(&json!({})).is_err());
        assert!(pane_number(&json!({ "pane": "x" })).is_err());
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
        let info = parse_pane_info(
            1,
            &json!({
                "id": 5, "cols": 80, "rows": 24, "command": "htop", "title": null,
                "mouse": "any", "focus_tracking": true
            }),
        );
        assert_eq!(info.number, 2, "1-based number is index + 1");
        assert_eq!(info.id, 5);
        assert_eq!(info.mouse.as_deref(), Some("any"));
        assert!(info.focus_tracking);
        // A resting pane: neither key present -> the resting defaults (None / false), never a panic.
        let resting = parse_pane_info(0, &json!({ "id": 1, "command": "bash", "title": null }));
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
            number: 3,
            id: 11,
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            active: false,
            shell: None,
            exit_status: None,
            mouse: None,
            focus_tracking: false,
            images: vec![],
            agent: None,
        };
        let events = vec![
            json!({ "type": "pane_job_changed", "pane": 11 }),
            json!({ "type": "pane_closed", "pane": 4 }),
            json!({ "type": "window_selected", "window": "build" }),
        ];

        assert_eq!(
            render_events(&events, std::slice::from_ref(&live)),
            "  pane_job_changed: pane 3 (id 11)\n  pane_closed: pane ? (id 4, gone since the pane \
             list was read)\n  window_selected: window build\n",
        );
    }

    #[test]
    fn pane_summary_surfaces_mouse_and_focus_tracking() {
        let tracking = PaneInfo {
            number: 2,
            id: 7,
            title: String::new(),
            command: "vim".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
            active: false,
            shell: None,
            exit_status: None,
            mouse: Some("any".to_owned()),
            focus_tracking: true,
            images: vec![],
            agent: None,
        };
        let summary = pane_summary(&tracking);
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
        let resting = pane_summary(&resting);
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
            number: 1,
            id: 3,
            title: String::new(),
            command: "bash".to_owned(),
            cols: 80,
            rows: 24,
            notification: None,
            bell: 0,
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
        let summary = pane_summary(&claimed);
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
        let summary = pane_summary(&reported);
        assert!(
            summary.contains("agent: state=working name=claude source=hook:claude seq=5"),
            "an authority is told from an inference: {summary}",
        );
        let quiet = pane_summary(&PaneInfo {
            agent: None,
            ..reported
        });
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
