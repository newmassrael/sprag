//! An AGENT that does what the hooks it was launched with tell it to — the stand-in every test of
//! sprag's per-launch instrumentation needs.
//!
//! # Why this is a program and not a shell script
//!
//! The thing under test is a DOCUMENT: sprag appends `--settings <json>` to an agent's argv, and the
//! claim is that a real agent can read that document and run what it names. A stand-in that pulled
//! the command out with `sed` would agree with the producer by construction — both would be reading
//! the same characters the same way — and the one failure it exists to catch (sprag emitting a
//! document no agent can act on) would come back green. This parses the JSON the way its reader
//! does, so a malformed or mis-nested document fails here.
//!
//! # Why it is DRIVEN rather than timed
//!
//! Each line on stdin is one event to raise. That makes a turn's boundaries the TEST's to place: a
//! test can start a turn, wait for the daemon to have been told, and end it — where a stand-in that
//! fired its own events on a timer would make every assertion about it a race. It is also what lets
//! a test build the case this whole mechanism exists for: a turn that begins and ends between two
//! samples of the screen.
//!
//! After running an event's hooks it prints `<event> done` and flushes, so a test can wait on the
//! pane's SCREEN for the fact that the report has already been made.
//!
//! # What it is faithful about, and what it is not
//!
//! Faithful: where the hooks live in the document (`hooks.<event>[].hooks[]`), that an entry's
//! `command` is a shell command line, that the payload arrives on the hook's STDIN as an object
//! naming `hook_event_name`, and that every entry configured for an event runs.
//!
//! Not faithful, deliberately: it merges no other settings source, enforces no `timeout`, and knows
//! nothing about matchers. Each of those belongs to a claim this stand-in is not used for, and a
//! stand-in that pretended to have them would be a second, worse implementation of an agent nobody
//! can consult.
//!
//! # ⚠⚠⚠⚠ The second document: `--mcp-config`, and why this program SPEAKS the protocol
//!
//! Register item 444 gave a pane's agent one more thing at launch — the MCP server of the image
//! that made its pane. That claim has the same shape as the hooks one and the same way of coming
//! back green while broken: a stand-in that merely READ the injected path would agree with the
//! producer by construction, and would say nothing about whether the document names a server an
//! agent can actually start and talk to.
//!
//! So this spawns what the document names and speaks newline-delimited JSON-RPC to it —
//! `initialize`, then `tools/list` — and prints what came back on the PANE, where the daemon's own
//! end-to-end gate reads it. A server that cannot be spawned, a document nested one level wrong, a
//! path that is not there: each fails HERE, with a sentence, which is the entire reason this is a
//! program.
//!
//! ⚠ Not faithful, deliberately and for the hooks half's reason: it starts servers serially, holds
//! no session open, sends no `notifications/initialized`, and takes ONE value per occurrence of the
//! flag where the real agent's is variadic. sprag emits one document naming one server; a stand-in
//! that guessed at the rest would be guessing about an agent nobody can consult.
//!
//! # Why it is a binary of THIS crate and not a workspace member of its own
//!
//! Because that is the only spelling cargo makes a PROMISE about. `tests/wire_client.rs` needs this
//! program to exist as a file on disk, and cargo sets `CARGO_BIN_EXE_<name>` — and builds the
//! binary — for every binary of the package an integration test belongs to, under any target
//! filter. It was a member crate first, and the test found it by taking `CARGO_BIN_EXE_sprag-term`'s
//! directory and joining a guessed name onto it. That guess is not a dependency cargo can see:
//! `cargo test -p sprag-host --test wire_client` builds no other package's binaries, so the file was
//! there only for whoever had run `cargo build` earlier. CI had not, and the test failed on the one
//! machine that was honest about it. Living here, the dependency is declared by construction.

use std::io::{BufRead, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

fn main() {
    let settings = match settings_from(std::env::args().skip(1)) {
        Ok(settings) => settings,
        Err(reason) => {
            // Printed to the PANE rather than to stderr: a stand-in inside a pty has one output, and
            // a test that has to diagnose it reads the screen.
            println!("agent-peer: {reason}");
            let _ = std::io::stdout().flush();
            // Still park, so the pane stays open and a test can read the sentence above rather than
            // meeting a pane that vanished.
            park();
            return;
        }
    };
    println!("agent-peer ready");
    let _ = std::io::stdout().flush();

    // ⚠ SEPARATE FROM THE SETTINGS ARM AND AFTER IT, because the two documents are independent on
    // the producing side too: a launch may carry either, both or neither, and a stand-in that made
    // one a precondition of the other could not tell which arm had failed.
    match mcp_from(std::env::args().skip(1)) {
        Ok(servers) => {
            for (name, entry) in servers {
                match ask(&entry) {
                    Ok(said) => println!("agent-peer mcp {name} {said}"),
                    Err(reason) => println!("agent-peer mcp {name}: {reason}"),
                }
            }
        }
        Err(reason) => println!("agent-peer mcp: {reason}"),
    }
    let _ = std::io::stdout().flush();

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let event = line.trim();
        if event.is_empty() {
            continue;
        }
        if event == "exit" {
            break;
        }
        let ran = raise(&settings, event);
        println!("{event} done ({ran})");
        let _ = std::io::stdout().flush();
    }
}

/// The settings document this launch was given: `--settings <file-or-json>`, exactly the flag's own
/// contract. Both forms are accepted because both are what the flag means, and a stand-in that took
/// only the one sprag happens to emit could not notice sprag switching to the other.
fn settings_from(mut args: impl Iterator<Item = String>) -> Result<Value, String> {
    let mut document = None;
    while let Some(arg) = args.next() {
        if arg == "--settings" {
            document = Some(args.next().ok_or("--settings with no value")?);
        } else if let Some(inline) = arg.strip_prefix("--settings=") {
            document = Some(inline.to_owned());
        }
    }
    // LAST wins, which is what a flag repeated on one command line means everywhere else.
    let document = document.ok_or("no --settings on this launch")?;
    let text = if document.trim_start().starts_with('{') {
        document
    } else {
        std::fs::read_to_string(&document).map_err(|error| format!("{document}: {error}"))?
    };
    serde_json::from_str(&text).map_err(|error| format!("the settings are not JSON: {error}"))
}

/// The MCP servers this launch was given: `--mcp-config <file-or-json>`, read the way the settings
/// document is and for the same reasons — both spellings, last wins, and a missing flag is an ERROR
/// rather than an empty roster, so a launch that was never injected into cannot read as one injected
/// with nothing.
///
/// Answers `(name, entry)` in the document's own order, because what an agent starts is every server
/// the document names and not just the first.
fn mcp_from(mut args: impl Iterator<Item = String>) -> Result<Vec<(String, Value)>, String> {
    let mut document = None;
    while let Some(arg) = args.next() {
        if arg == "--mcp-config" {
            document = Some(args.next().ok_or("--mcp-config with no value")?);
        } else if let Some(inline) = arg.strip_prefix("--mcp-config=") {
            document = Some(inline.to_owned());
        }
    }
    let document = document.ok_or("no --mcp-config on this launch")?;
    let text = if document.trim_start().starts_with('{') {
        document
    } else {
        std::fs::read_to_string(&document).map_err(|error| format!("{document}: {error}"))?
    };
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the mcp config is not JSON: {error}"))?;
    let servers = parsed
        .get("mcpServers")
        .and_then(Value::as_object)
        .ok_or("the mcp config names no `mcpServers` object")?;
    Ok(servers
        .iter()
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect())
}

/// Start the server `entry` names and ask it what it is — `initialize`, then `tools/list`.
///
/// Answers the one line a test reads off the pane: which BUILD the server said it was, and that it
/// served a roster at all. The build is the substance (register item 444: two images published the
/// same identity and nothing could tell them apart); the roster is the control, because a server
/// that answered `initialize` and then nothing would otherwise read as a working one.
///
/// Every failure is a SENTENCE. A stand-in that returned an empty string for a server it could not
/// start would put the daemon's gate in front of a silence, which is the shape this whole binary
/// exists to avoid.
fn ask(entry: &Value) -> Result<String, String> {
    let command = entry
        .get("command")
        .and_then(Value::as_str)
        .ok_or("the entry names no command")?;
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut child = Command::new(command)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{command}: {error}"))?;
    let mut sink = child.stdin.take().ok_or("no stdin on the server")?;
    let mut source = std::io::BufReader::new(child.stdout.take().ok_or("no stdout on the server")?);

    let hello = call(
        &mut sink,
        &mut source,
        1,
        "initialize",
        serde_json::json!({ "protocolVersion": "2025-06-18", "capabilities": {},
                            "clientInfo": { "name": "sprag-agent-peer", "version": "0" } }),
    )?;
    let version = hello
        .pointer("/result/serverInfo/version")
        .and_then(Value::as_str)
        .ok_or("initialize answered no serverInfo version")?
        .to_owned();
    let roster = call(&mut sink, &mut source, 2, "tools/list", Value::Null)?;
    let tools = roster
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .ok_or("tools/list answered no roster")?
        .len();

    // Closing stdin is how a stdio server is told to go; the wait keeps a finished pane from
    // holding a zombie the daemon's own process reader would then report on.
    drop(sink);
    let _ = child.wait();
    Ok(format!("version={version} tools={tools}"))
}

/// One JSON-RPC round trip over a stdio server: write the request, read lines until the reply
/// carrying `id` — every other line is a notification, which a server may send at any time.
fn call(
    sink: &mut impl Write,
    source: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let mut request = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if !params.is_null() {
        request["params"] = params;
    }
    writeln!(sink, "{request}").map_err(|error| format!("{method}: {error}"))?;
    sink.flush().map_err(|error| format!("{method}: {error}"))?;
    loop {
        let mut line = String::new();
        if source
            .read_line(&mut line)
            .map_err(|error| format!("{method}: {error}"))?
            == 0
        {
            return Err(format!("{method}: the server closed without answering"));
        }
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(message);
        }
    }
}

/// Run every hook configured for `event`, with the payload an agent sends. Answers how many ran, so
/// a test can tell "the document named nothing for this event" from "the hooks ran".
fn raise(settings: &Value, event: &str) -> usize {
    let payload = serde_json::json!({ "hook_event_name": event }).to_string();
    let mut ran = 0;
    for command in commands_for(settings, event) {
        // `sh -c` because an entry's `command` is a shell command LINE, not an argv — the agent's
        // own contract, and what makes an absolute path with spaces the installer's problem rather
        // than a difference between this stand-in and the real thing.
        let Ok(mut child) = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut sink) = child.stdin.take() {
            let _ = sink.write_all(payload.as_bytes());
        }
        let _ = child.wait();
        ran += 1;
    }
    ran
}

/// Every `command` the document configures for `event`, in order.
fn commands_for(settings: &Value, event: &str) -> Vec<String> {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| entry.get("command").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Hold the pane open with nothing to say — a failed launch a test can still read.
fn park() {
    let mut sink = Vec::new();
    let _ = std::io::stdin().lock().read_until(0, &mut sink);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The flag contract, which is the only thing this stand-in must be right about.**
    ///
    /// It exists to catch sprag emitting a document no agent can act on, and it can only do that if
    /// it reads the flag the way an agent does. Both spellings, because sprag emits the separated
    /// one and a stand-in that knew only that could not notice sprag switching; LAST wins, because
    /// that is what a repeated flag means everywhere else; and a missing flag is an ERROR rather
    /// than an empty document, so a launch that was never instrumented cannot read as one that was
    /// instrumented with nothing.
    #[test]
    fn the_settings_flag_is_read_the_way_an_agent_reads_it() {
        let inline = |args: &[&str]| {
            settings_from(args.iter().map(|arg| (*arg).to_owned()))
                .map(|doc| doc["mark"].as_str().unwrap_or_default().to_owned())
        };
        assert_eq!(
            inline(&["--settings", r#"{"mark":"separated"}"#]).as_deref(),
            Ok("separated"),
        );
        assert_eq!(
            inline(&["--settings={\"mark\":\"joined\"}"]).as_deref(),
            Ok("joined"),
        );
        assert_eq!(
            inline(&[
                "--settings",
                r#"{"mark":"first"}"#,
                "--settings",
                r#"{"mark":"last"}"#,
            ])
            .as_deref(),
            Ok("last"),
            "a repeated flag means the last one, as it does everywhere else",
        );
        assert!(
            inline(&["--model", "sonnet"]).is_err(),
            "a launch with no settings is not a launch instrumented with nothing",
        );
        assert!(
            inline(&["--settings"]).is_err(),
            "the flag without its value is refused rather than read as absent",
        );
        assert!(
            inline(&["--settings", "{not json"]).is_err(),
            "a document that is not JSON fails HERE, which is the failure this exists to find",
        );
    }

    /// **The MCP flag contract**, the settings one's twin and asserted for the same reason: this
    /// stand-in exists to catch sprag emitting a document no agent can act on, which it can only do
    /// if it reads the flag the way an agent does.
    ///
    /// ⚠ The last case is the one with teeth. A document that parses but nests the servers
    /// elsewhere is EXACTLY the defect a reader written from the producer's own idea of the shape
    /// would wave through, and it would come back as *the injection works* while a real agent
    /// started nothing.
    #[test]
    fn the_mcp_flag_is_read_the_way_an_agent_reads_it() {
        let named = |args: &[&str]| {
            mcp_from(args.iter().map(|arg| (*arg).to_owned())).map(|servers| {
                servers
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            named(&["--mcp-config", r#"{"mcpServers":{"separated":{}}}"#]),
            Ok(vec!["separated".to_owned()]),
        );
        assert_eq!(
            named(&["--mcp-config={\"mcpServers\":{\"joined\":{}}}"]),
            Ok(vec!["joined".to_owned()]),
        );
        assert_eq!(
            named(&[
                "--mcp-config",
                r#"{"mcpServers":{"first":{}}}"#,
                "--mcp-config",
                r#"{"mcpServers":{"last":{}}}"#,
            ]),
            Ok(vec!["last".to_owned()]),
            "a repeated flag means the last one, as it does everywhere else",
        );
        assert!(
            named(&["--settings", "{}"]).is_err(),
            "a launch with no server named is not a launch injected with nothing",
        );
        assert!(
            named(&["--mcp-config"]).is_err(),
            "the flag without its value is refused rather than read as absent",
        );
        assert!(
            named(&["--mcp-config", "{not json"]).is_err(),
            "a document that is not JSON fails HERE, which is the failure this exists to find",
        );
        assert!(
            named(&["--mcp-config", r#"{"servers":{"wrong-nesting":{}}}"#]).is_err(),
            "⚠⚠ and so does one that parses while naming its servers somewhere an agent will not \
             look",
        );
    }

    /// **Where the hooks live in the document** — the other half of the same faithfulness.
    ///
    /// A stand-in that looked one level too high or too low would run nothing for a document that
    /// is perfectly correct, and the end-to-end that drives it would report the producer broken.
    /// Order is asserted because an agent runs the entries as written, and the unknown event is the
    /// control: without it, a reader that returned every command in the file would pass.
    #[test]
    fn the_commands_for_an_event_are_the_ones_the_document_nests_under_it() {
        let doc: Value = serde_json::from_str(
            r#"{"hooks":{
                 "Stop":[{"hooks":[{"command":"first"},{"command":"second"}]}],
                 "Notification":[{"hooks":[{"command":"elsewhere"}]}]
               }}"#,
        )
        .expect("the fixture is JSON");
        assert_eq!(commands_for(&doc, "Stop"), vec!["first", "second"]);
        assert_eq!(commands_for(&doc, "Notification"), vec!["elsewhere"]);
        assert!(
            commands_for(&doc, "PreToolUse").is_empty(),
            "an event the document says nothing about runs nothing",
        );
        assert!(
            commands_for(&Value::Null, "Stop").is_empty(),
            "and neither does a document with no hooks at all",
        );
    }
}
