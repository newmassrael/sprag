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
