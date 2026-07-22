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
//! JSON-RPC 2.0 on stdin/stdout. It advertises seven self-describing tools —
//! `list_panes`, `read_pane`, `read_last_command`, `read_pane_links`, `read_pane_images`,
//! `write_pane`, `send_keys` — so an
//! agent *immediately* understands "read/write a sibling pane" without reading any sprag
//! source. Each tool call bridges to the host wire via [`sprag_rpc::HostConn`],
//! addressing panes with the [`sprag_host::wire`] path SSOT.
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
use std::time::Duration;

use serde_json::{Value, json};
use sprag_host::wire::{
    FULL_TEXT_SLOT, KEY_ACTION, LAST_COMMAND_SLOT, LINKS_SLOT, PANES_SLOT, TEXT_ACTION,
};
use sprag_host::{mux_action_path, pane_input_path};
use sprag_rpc::HostConn;

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
            its result, type text into a pane, or send keys. \
            Call `list_panes` first to see the pane numbers (1 = first pane). \
            \"pane 2\" means the second pane in that list. If a tool reports it is not \
            inside a sprag terminal, these tools do not apply to this session."
    })
}

/// The seven self-describing tools. Descriptions are written for an agent so a request
/// like "type xxx into pane 2" maps directly onto the `write_pane` tool.
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
        "read_pane" => tool_read_pane(&args),
        "read_last_command" => tool_read_last_command(&args),
        "read_pane_links" => tool_read_pane_links(&args),
        "read_pane_images" => tool_read_pane_images(&args),
        "write_pane" => tool_write_pane(&args),
        "send_keys" => tool_send_keys(&args),
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
    let mut out = format!(
        "  pane {}: id={} {}x{} command={} title={}\n",
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
    out
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
    ancestor_sock()
}

/// Walk the parent-process chain from our own PID, returning the socket path from the
/// first ancestor whose environment holds `SPRAG_HOST_RPC_SOCK`. Bounded so a broken
/// `/proc` (or a PID cycle) can never loop forever.
fn ancestor_sock() -> Option<PathBuf> {
    let mut pid = std::process::id();
    for _ in 0..64 {
        let ppid = read_ppid(pid)?;
        if ppid == 0 || ppid == pid {
            return None;
        }
        if let Some(path) = read_proc_env(ppid, SOCK_ENV) {
            return Some(PathBuf::from(path));
        }
        pid = ppid;
    }
    None
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
    fn tools_list_advertises_the_seven_tools_with_object_schemas() {
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
                "read_pane",
                "read_last_command",
                "read_pane_links",
                "read_pane_images",
                "write_pane",
                "send_keys"
            ]
        );
        for tool in tools["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 10);
        }
        // write_pane requires pane + text (the "type xxx into pane 2" path).
        let write = tools["tools"].as_array().unwrap()[5].clone();
        assert_eq!(write["inputSchema"]["required"], json!(["pane", "text"]));
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
            shell: None,
            exit_status: None,
            mouse: Some("any".to_owned()),
            focus_tracking: true,
            images: vec![],
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
