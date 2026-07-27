//! `sprag-smoke` — the LIVE headless smoke, as a committed tool instead of a script rewritten from
//! memory every round.
//!
//! It boots an isolated `sprag-term` daemon and a real `sprag-gui` against it, drives the client
//! through its own scene RPC, and asserts what actually got painted and announced. Every front that
//! has needed this so far re-derived the same six or seven facts — which env var drives which socket,
//! that `scene/invoke` wants an `args` key even for a verb that takes none, that synthetic winit
//! input never lands headless — and each rediscovery cost iterations. They are encoded here once.
//!
//! ## Running it
//!
//! ```text
//! cargo build -p sprag-gui                       # REBUILD FIRST: cargo test does not refresh the binary
//! xvfb-run -a ./target/debug/sprag-smoke
//! ```
//!
//! Xvfb is the caller's to provide, not this tool's to spawn: a smoke that manages its own display
//! server hides the one failure that matters most (the renderer could not start), and `xvfb-run`
//! already owns that lifecycle properly.
//!
//! Exit code is the number of failed checks, so it composes with a shell `&&`. It is NOT a
//! `cargo test`: it needs a built binary, a software Vulkan stack and an X display, so folding it
//! into the gate would make the gate fail on machines where nothing is wrong.
//!
//! ## The renderer, which is the part that is not guessable
//!
//! sprag-gui renders through pinion → vello → wgpu, which needs a Vulkan DEVICE. Under Xvfb a
//! GPU ICD has no surface to bind and wgpu reports no suitable device, which reads as a sprag bug
//! and is not one. The fix is Mesa's software ICD (lavapipe), forced here through
//! `VK_ICD_FILENAMES` + `WGPU_BACKEND`. The GL/llvmpipe backend is NOT an alternative — vello
//! rejects it.
//!
//! ## Three sockets, three variables
//!
//! * `SPRAG_HOST_RPC_SOCK` — the daemon's own socket (what `sprag-term` binds, what the CLI uses).
//! * `SPRAG_GUI_HOST_SOCK` — where the GUI looks for that host.
//! * `SPRAG_RPC_SOCK` — the GUI's OWN scene socket, which is the one this tool drives.
//!
//! They are separate because the GUI is a client of the host AND a server of its scene; pointing two
//! of them at one path is the mistake that produces a client talking to itself.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_rpc::HostConn;

/// How long any wait-for-a-condition may take before the smoke calls it a failure. Generous, because
/// a software rasteriser under Xvfb is slow to reach its first frame and a flaky timeout would be a
/// worse lie than a slow pass.
const PATIENCE: Duration = Duration::from_secs(60);

/// How often a wait re-reads the condition.
const POLL: Duration = Duration::from_millis(100);

fn main() -> ExitCode {
    let mut report = Report::default();
    match Smoke::boot() {
        Ok(mut smoke) => {
            check_the_palette_opens_over_rpc(&mut smoke, &mut report);
            check_a_command_runs_from_a_palette_row(&mut smoke, &mut report);
            check_a_pane_can_be_created_and_closed(&mut smoke, &mut report);
            check_a_window_closes_under_a_live_client(&mut smoke, &mut report);
            // LAST, and it must stay last: it destroys the session this client is attached to, so
            // the client leaves and every check after it would be asserting against a dead socket.
            check_killing_the_attached_session_ends_the_client(&mut smoke, &mut report);
        }
        Err(error) => {
            eprintln!("FAIL  the smoke could not boot: {error}");
            report.failed.push("boot".to_owned());
        }
    }
    report.finish()
}

// ─── The checks ──────────────────────────────────────────────────────────────────────────────────

/// The palette opens on a REQUEST, paints a content-sized panel, and announces a modal dialog.
///
/// The `open` verb is what makes this reachable at all: the palette's only other entry is a chord,
/// and synthetic key input does not drain headless — so before the verb existed, nothing in this
/// function could run.
fn check_the_palette_opens_over_rpc(smoke: &mut Smoke, report: &mut Report) {
    report.check(
        "the palette starts unpainted",
        !smoke.tags().contains_key("sprag_palette_panel"),
    );

    // Drive real focus first. The GUI's boot focus request never drains under Xvfb (no winit input
    // tick), so the within-app focus starts genuinely absent — and the palette CAPTURES the focused
    // pane, so asserting on its catalog without this would be asserting about nothing.
    let _ = smoke.call("focus/set", json!({ "tag": "sprag_gui.pane.0" }));
    report.check(
        "pane 0 holds the within-app focus",
        smoke
            .call("focus/get", json!({}))
            .ok()
            .and_then(|value| value["focused"].as_str().map(str::to_owned))
            .as_deref()
            == Some("sprag_gui.pane.0"),
    );

    report.check(
        "scene/invoke open is accepted",
        smoke.invoke("sprag_palette", "open", Value::Null) == Ok(Value::Bool(true)),
    );
    let painted = match smoke.wait_for_tag("sprag_palette_panel") {
        Ok(tags) => tags,
        Err(error) => {
            report.check(&format!("the palette panel paints: {error}"), false);
            return;
        }
    };
    report.check("the palette OPENED headlessly (no chord pressed)", true);

    let rows = smoke
        .query("sprag_palette", "row_count")
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let drawn = rows.min(MAX_VISIBLE_ROWS);
    report.check(&format!("the frozen catalog has rows ({rows})"), rows > 0);

    // RECTS, not just tags: a node can exist at h=0, and the content sizing is the claim.
    let panel = painted.get("sprag_palette_panel").and_then(|n| n.rect);
    let want_h = PANEL_PADDING * 2 + FIELD_H + ROW_GAP + drawn as u32 * (ROW_H + ROW_GAP);
    report.check(
        &format!("the panel measures {PANEL_W}x{want_h} for {drawn} painted rows (got {panel:?})"),
        panel == Some((PANEL_W, want_h)),
    );
    let box_rect = painted.get("sprag_palette_rows").and_then(|n| n.rect);
    let want_rows = (
        PANEL_W - PANEL_PADDING * 2,
        drawn as u32 * ROW_H + (drawn as u32 - 1) * ROW_GAP,
    );
    report.check(
        &format!("the rows container measures {want_rows:?} (got {box_rect:?})"),
        box_rect == Some(want_rows),
    );

    // The prompt glyph is what makes the empty query field visible at all — a live screenshot once
    // caught its absence, and nothing but a paint assertion catches it again.
    report.check(
        "the prompt glyph is painted",
        painted
            .get("sprag_palette_input")
            .is_some_and(|node| node.text.iter().any(|t| t == "\u{203a}")),
    );
    report.check(
        "the focused pane was captured (its pane commands are offered)",
        painted
            .get("sprag_palette_rows")
            .is_some_and(|node| node.text.iter().any(|t| t == "Find in scrollback")),
    );

    // ...and the ACCESSIBLE tree, which is the half a pixel assertion cannot reach.
    let access = smoke.access();
    let dialog = access.get("sprag_palette_panel");
    report.check(
        "the palette announces a MODAL dialog",
        dialog.is_some_and(|node| node["role"] == "dialog" && node["modal"] == json!(true)),
    );
    report.check(
        "...with bounds the shell resolved from its painted tag",
        dialog.is_some_and(|node| node.get("bounds").is_some()),
    );
    report.check(
        "the query field announces an editable combobox",
        access
            .get("sprag_palette_query")
            .is_some_and(|node| node["role"] == "combobox"),
    );
    report.check(
        "the rows announce a named listbox",
        access.get("sprag_palette_rows").is_some_and(|node| {
            node["role"] == "listbox" && node["name"] == json!("Matching commands")
        }),
    );
    report.check(
        &format!("one accessible option per PAINTED row ({drawn})"),
        access.values().filter(|n| n["role"] == "option").count() == drawn,
    );
}

/// A palette row RUNS its command over the RPC `execute` path, end to end.
///
/// Watched through a CLIENT-side effect (`Find in scrollback` paints the find bar) rather than
/// through the palette merely closing — a dismiss closes it too, so only the effect distinguishes
/// "the reducer ran the command" from "the panel went away".
fn check_a_command_runs_from_a_palette_row(smoke: &mut Smoke, report: &mut Report) {
    let Some(at) = smoke.row_named("Find in scrollback") else {
        report.check("the palette offers `Find in scrollback` to run", false);
        return;
    };
    let _ = smoke.invoke("sprag_palette", "select", json!(at));
    report.check(
        "select moves the cursor onto that row",
        smoke.query("sprag_palette", "cursor_command") == Ok(json!("Find in scrollback")),
    );
    report.check(
        "execute reports the title it armed",
        smoke.invoke("sprag_palette", "execute", Value::Null) == Ok(json!("Find in scrollback")),
    );
    match smoke.wait_for_tag("sprag_find") {
        Ok(tags) => {
            report.check("the RPC execute path RAN the command", true);
            report.check(
                "running a command closed the palette",
                !tags.contains_key("sprag_palette_panel"),
            );
        }
        Err(error) => report.check(
            &format!("the find bar the command opens paints: {error}"),
            false,
        ),
    }
}

/// A pane is created and closed from the PALETTE, through the client's own host connection.
///
/// Driven through the palette rather than straight at the daemon on purpose, and the reason is the
/// trap that cost this check its first version: the GUI creates its OWN session, so an unscoped
/// connection to the daemon spawns into the daemon's boot session — a pane that exists, that the
/// client is not attached to, and that therefore never appears. Going through the palette means the
/// request rides the client's own scoped connection, which is also the path a user takes.
///
/// The kill half is the whole destructive arc end to end: the row activates into a CONFIRMATION,
/// nothing is destroyed until it is answered, and answering it is what closes the pane.
fn check_a_pane_can_be_created_and_closed(smoke: &mut Smoke, report: &mut Report) {
    let before = smoke.pane_count();
    report.check(
        &format!("the window starts with {before} pane(s)"),
        before > 0,
    );

    if !smoke.run_palette_row("Split into a new pane", report) {
        return;
    }
    let grown = smoke.wait_for(|s| (s.pane_count() > before).then(|| s.pane_count()));
    report.check(
        &format!("the split reached the client's tiling ({grown:?})"),
        grown.is_ok(),
    );
    if grown.is_err() {
        return;
    }

    // Now kill one. The row is DESTRUCTIVE, so it must not act on activation alone.
    if !smoke.run_palette_row("Kill pane", report) {
        return;
    }
    let prompt = smoke
        .query("sprag_confirm", "prompt")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned));
    report.check(
        &format!("a kill row asks before it acts (prompt: {prompt:?})"),
        prompt.is_some_and(|p| p.contains('?')),
    );
    report.check(
        "and nothing is destroyed by the asking",
        smoke.pane_count() > before,
    );

    report.check(
        "the prompt is answerable over RPC",
        smoke.invoke("sprag_confirm", "accept", Value::Null).is_ok(),
    );
    let shrunk = smoke.wait_for(|s| (s.pane_count() == before).then(|| s.pane_count()));
    report.check(
        &format!("answering it closed the pane ({shrunk:?})"),
        shrunk.is_ok(),
    );
}

/// A WINDOW opening and closing reaches the tab strip a live client is painting.
///
/// The window vertical was wire-proven long before this: the registry, the wire actions and the CLI
/// all had tests. What none of them could answer is whether an ATTACHED, rendering client notices —
/// the client mirrors the window list on a poll, and a mirror that failed to re-adopt it would leave
/// a tab for a window that no longer exists, with every test still green. Only a real GUI painting
/// real tabs closes that.
///
/// The new window's name is DISCOVERED from the strip rather than predicted, so this asserts what
/// the client shows rather than re-deriving the host's naming scheme — which is the thing a smoke is
/// for, and the thing a re-derivation would quietly get wrong.
fn check_a_window_closes_under_a_live_client(smoke: &mut Smoke, report: &mut Report) {
    let before = smoke.tabs();
    report.check(
        &format!("the strip starts with one tab ({before:?})"),
        before.len() == 1,
    );

    if !smoke.run_palette_row("New window", report) {
        return;
    }
    let Ok(grown) = smoke.wait_for(|s| {
        let tabs = s.tabs();
        (tabs.len() > before.len()).then_some(tabs)
    }) else {
        report.check("the new window reaches the client's tab strip", false);
        return;
    };
    report.check(
        &format!("the new window painted its own tab ({grown:?})"),
        true,
    );

    let Some(born) = grown.iter().find(|name| !before.contains(name)).cloned() else {
        report.check("the new tab carries a name of its own", false);
        return;
    };

    // Kill it BY NAME through the palette — the same destructive arc a pane kill takes, so the
    // confirmation is proven for a window target too and not just assumed to behave alike.
    if !smoke.run_palette_row(&format!("Kill window {born}"), report) {
        return;
    }
    report.check(
        "killing a window asks first",
        smoke
            .query("sprag_confirm", "prompt")
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .is_some_and(|prompt| prompt.contains(&born)),
    );
    report.check(
        "and nothing closed by the asking",
        smoke.tabs().len() == grown.len(),
    );
    report.check(
        "the prompt is answerable over RPC",
        smoke.invoke("sprag_confirm", "accept", Value::Null).is_ok(),
    );

    let shrunk = smoke.wait_for(|s| {
        let tabs = s.tabs();
        (!tabs.contains(&born)).then_some(tabs)
    });
    report.check(
        &format!("the closed window left the live client's strip ({shrunk:?})"),
        shrunk.is_ok_and(|tabs| tabs == before),
    );
}

/// Killing the session this client is ATTACHED to ends the client — tmux's rule that a client leaves
/// when it can no longer serve its session, under the default `detach-on-destroy`.
///
/// The last unproven step of the destroy arc. The poll thread's classification of a dead session was
/// unit-tested against a fake socket; that a REAL rendering process, mid-frame, actually leaves — and
/// does not sit painting a session that no longer exists — is a fact only a live client can settle.
///
/// The assertion is on the PROCESS, deliberately. There is no pixel to read here: the correct
/// outcome is that there are no more pixels, and a window that lingers empty would look identical to
/// one still working over any scene query this tool could make.
///
/// The session is DISCOVERED ([`Smoke::attached_session`]), never assumed to be the first one the
/// palette lists — the daemon has its own boot session and a GUI gets a second, so the first
/// `Kill session` row belongs to somebody else. That mistake is a convincing false alarm: the
/// client keeps running, exactly as it should, and the check calls it a failure to detach.
fn check_killing_the_attached_session_ends_the_client(smoke: &mut Smoke, report: &mut Report) {
    let Some(mine) = smoke.attached_session() else {
        report.check("the client says which session it is attached to", false);
        return;
    };
    // `run_palette_row` already reports whether the row was offered and whether it ran, so the
    // discovered name needs no assertion of its own beyond appearing in those lines.
    if !smoke.run_palette_row(&format!("Kill session {mine}"), report) {
        return;
    }
    report.check(
        "killing a session asks first, like every other destructive row",
        smoke
            .query("sprag_confirm", "prompt")
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .is_some_and(|prompt| prompt.contains('?')),
    );
    report.check(
        "the client is still alive while the prompt stands",
        !smoke.gui_exited(),
    );

    // From here the socket is expected to die, so nothing may assert through it again.
    let _ = smoke.invoke("sprag_confirm", "accept", Value::Null);
    report.check(
        "the client LEFT when its session was destroyed",
        smoke.wait_for(|s| s.gui_exited().then_some(())).is_ok(),
    );
}

// ─── The paint constants the assertions predict ──────────────────────────────────────────────────
//
// Spelled here rather than imported: sprag-gui is a BIN crate with no library to import them from,
// and a smoke that read the same constant the code did would assert that the code equals itself.
// Independent literals are the point — if the palette's geometry changes, this must be updated
// deliberately, which is the review the change deserves.

/// The palette panel's width in logical pixels.
const PANEL_W: u32 = 460;
/// The panel's inner padding on every edge.
const PANEL_PADDING: u32 = 12;
/// The query field's height.
const FIELD_H: u32 = 40;
/// One command row's height.
const ROW_H: u32 = 28;
/// The gap between the field and the rows, and between rows.
const ROW_GAP: u32 = 4;
/// The most rows the palette paints at once.
const MAX_VISIBLE_ROWS: usize = 10;

// ─── The harness ─────────────────────────────────────────────────────────────────────────────────

/// One painted node, flattened out of a `scene/snapshot` tree.
#[derive(Debug, Default)]
struct Painted {
    /// The node's laid-out `(w, h)`, when it has one.
    rect: Option<(u32, u32)>,
    /// Every string painted anywhere in this node's SUBTREE.
    ///
    /// The subtree, not the node: a widget row is a `Container` carrying the tag while the label is
    /// the `content` of an untagged `Text` CHILD, so reading `content` off the matched node itself
    /// finds nothing. That shape cost an iteration once; collecting the subtree is the fix.
    text: Vec<String>,
}

/// A booted daemon + GUI, and the scene connection to drive them.
struct Smoke {
    daemon: Child,
    gui: Child,
    conn: HostConn,
    /// The GUI's scene socket, for the teardown unlink.
    gui_sock: PathBuf,
    /// The daemon's socket.
    host_sock: PathBuf,
    /// The isolated state dir, removed on the way out.
    state: PathBuf,
}

impl Smoke {
    /// Boot an isolated daemon, a GUI against it, and connect to the GUI's scene socket.
    fn boot() -> io::Result<Self> {
        let target = std::env::current_exe()?
            .parent()
            .ok_or_else(|| io::Error::other("the smoke binary has no directory"))?
            .to_path_buf();
        // SHORT paths: an AF_UNIX address is capped at 108 bytes, and a path under the target
        // directory of a deep checkout is comfortably past it.
        let unique = std::process::id();
        let host_sock = PathBuf::from(format!("/tmp/sp{unique}h.sock"));
        let gui_sock = PathBuf::from(format!("/tmp/sp{unique}g.sock"));
        let state = PathBuf::from(format!("/tmp/sp{unique}state"));
        std::fs::create_dir_all(&state)?;

        let daemon = spawn(&target.join("sprag-term"), &host_sock, &gui_sock, &state)?;
        wait_for_path(&host_sock)?;
        let gui = spawn(&target.join("sprag-gui"), &host_sock, &gui_sock, &state)?;
        wait_for_path(&gui_sock)?;
        let conn = HostConn::connect(&gui_sock, PATIENCE)?;

        let mut smoke = Self {
            daemon,
            gui,
            conn,
            gui_sock,
            host_sock,
            state,
        };
        // The OS-focus gate: without this `os_focused_window` is null under Xvfb and anything that
        // reads it describes an unfocused window.
        let _ = smoke.call("scene/window_focus", json!({ "focused": true }));
        // The first pane painting is the real "the renderer came up" signal — a booted process that
        // never reaches a frame is the failure this tool exists to catch, and it must be reported as
        // a boot failure rather than as a hundred confusing check failures downstream.
        smoke
            .wait_for_tag("sprag_gui.pane.0")
            .map_err(io::Error::other)?;
        Ok(smoke)
    }

    /// One JSON-RPC call to the GUI's scene socket, with the server's error surfaced rather than
    /// swallowed — a wrong param shape answers `Invalid params` and looks exactly like "the call did
    /// nothing" to a caller that drops the result.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.conn
            .call(method, params)
            .map_err(|error| format!("{method}: {error}"))
    }

    /// Invoke `verb` on the external tagged `tag`.
    ///
    /// `args` is ALWAYS sent, including as `null`: the dispatcher requires the key even for a verb
    /// that takes nothing, and omitting it is `Invalid params`, not a default.
    fn invoke(&mut self, tag: &str, verb: &str, args: Value) -> Result<Value, String> {
        self.call(
            "scene/invoke",
            json!({ "path": format!("/{tag}/external/{verb}"), "args": args }),
        )
    }

    /// Query a value off the external tagged `tag`.
    fn query(&mut self, tag: &str, path: &str) -> Result<Value, String> {
        self.call(
            "scene/query",
            json!({ "path": format!("/{tag}/external/{path}") }),
        )
    }

    /// Every tagged node in the main window's PAINTED tree.
    ///
    /// `from: "paint"` is the displayed frame — real pixels; `"state"` is the pre-paint tree and
    /// would let a check pass on geometry that was never shown. The path is `/window[main]` with an
    /// EMPTY scene tail: a snapshot is a whole-tree dump, so a bare tag (or even `"/"`) is refused.
    fn tags(&mut self) -> std::collections::HashMap<String, Painted> {
        let mut out = std::collections::HashMap::new();
        if let Ok(value) = self.call(
            "scene/snapshot",
            json!({ "path": "/window[main]", "from": "paint" }),
        ) {
            walk(value.get("scene").unwrap_or(&value), &mut out);
        }
        out
    }

    /// The accessible tree, keyed by tag. Default-valued fields are OMITTED by the serializer, so an
    /// absent `modal` means false rather than unset.
    fn access(&mut self) -> std::collections::HashMap<String, Value> {
        let mut out = std::collections::HashMap::new();
        if let Ok(value) = self.call("scene/access", json!({}))
            && let Some(nodes) = value["nodes"].as_array()
        {
            for node in nodes {
                if let Some(tag) = node["tag"].as_str() {
                    out.insert(tag.to_owned(), node.clone());
                }
            }
        }
        out
    }

    /// How many pane tiles are painted.
    ///
    /// Counts the pane's own tag and not its `#grid` child: a pane paints several tagged nodes under
    /// one composite prefix, so a naive prefix count moves by more than one per pane and would make
    /// "one more pane" unstateable.
    fn pane_count(&mut self) -> usize {
        self.tags()
            .keys()
            .filter(|tag| {
                tag.strip_prefix("sprag_gui.pane.")
                    .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
            })
            .count()
    }

    /// The window names the tab strip is PAINTING, in tab order.
    ///
    /// Read off the tabs' own text rather than asked of the host: the claim under test is that the
    /// client's mirror reaches its pixels, and querying the host would answer with the very fact the
    /// mirror might have failed to adopt.
    fn tabs(&mut self) -> Vec<String> {
        let painted = self.tags();
        (0..)
            .map_while(|i| painted.get(&format!("sprag_gui.wtab.{i}")))
            .filter_map(|node| node.text.first().cloned())
            .collect()
    }

    /// Whether the GUI process has exited, without blocking on it.
    fn gui_exited(&mut self) -> bool {
        matches!(self.gui.try_wait(), Ok(Some(_)))
    }

    /// The name of the session this client is ATTACHED to.
    ///
    /// Read off the session rail's WAI-ARIA tablist — the tab carrying `selected` is the attached
    /// one — because the client is the only thing that knows. A daemon serves several sessions at
    /// once and a GUI gets its OWN, so the daemon's boot session is emphatically not it: pressing
    /// `Kill session 0` here kills a session this client never had, the client rightly keeps
    /// running, and a check that assumed otherwise reports a bug that is its own.
    ///
    /// The tab's accessible name leads with the session name and continues into its window count
    /// and directory (`1, 1 window, sprag`), so the name is the part before the first comma.
    fn attached_session(&mut self) -> Option<String> {
        self.access()
            .into_iter()
            .filter(|(tag, _)| tag.starts_with("sprag_gui.stab."))
            .find(|(_, node)| node["selected"] == json!(true))
            .and_then(|(_, node)| {
                let name = node["name"].as_str()?;
                Some(name.split(',').next().unwrap_or(name).trim().to_owned())
            })
    }

    /// Open the palette, put the cursor on the row titled `title`, and activate it.
    ///
    /// Returns whether the row was found and run; reports each step, so a failure says WHICH part of
    /// the chain broke rather than only that the effect never arrived.
    fn run_palette_row(&mut self, title: &str, report: &mut Report) -> bool {
        if self.invoke("sprag_palette", "open", Value::Null).is_err() {
            report.check(&format!("the palette opens to reach `{title}`"), false);
            return false;
        }
        if self.wait_for_tag("sprag_palette_panel").is_err() {
            report.check(&format!("the palette paints to reach `{title}`"), false);
            return false;
        }
        let Some(at) = self.row_named(title) else {
            report.check(&format!("the palette offers `{title}`"), false);
            let _ = self.invoke("sprag_palette", "send", json!("scrim:PointerUp"));
            return false;
        };
        let _ = self.invoke("sprag_palette", "select", json!(at));
        let ran = self.invoke("sprag_palette", "execute", Value::Null) == Ok(json!(title));
        report.check(&format!("the palette runs `{title}`"), ran);
        ran
    }

    /// Poll until `tag` is painted, answering the whole tree at that moment.
    fn wait_for_tag(
        &mut self,
        tag: &str,
    ) -> Result<std::collections::HashMap<String, Painted>, String> {
        let wanted = tag.to_owned();
        self.wait_for(move |smoke| {
            let tags = smoke.tags();
            tags.contains_key(&wanted).then_some(tags)
        })
        .map_err(|_| format!("timed out waiting for {tag}"))
    }

    /// Poll `condition` until it answers, or [`PATIENCE`] elapses.
    ///
    /// Waits on the CONDITION an assertion reads, never on a timer: a sleep long enough to pass on
    /// this machine is a flake on a slower one, and a flake is a bug rather than something to retry.
    fn wait_for<T>(
        &mut self,
        mut condition: impl FnMut(&mut Self) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(value) = condition(self) {
                return Ok(value);
            }
            if Instant::now() >= deadline {
                return Err("timed out".to_owned());
            }
            std::thread::sleep(POLL);
        }
    }

    /// The visible palette row whose title is `title`, by asking the palette itself rather than by
    /// reading the paint — the External's row list and the painted rows are one derivation, and this
    /// is the address `select` speaks.
    fn row_named(&mut self, title: &str) -> Option<u64> {
        let count = self.query("sprag_palette", "row_count").ok()?.as_u64()?;
        (0..count).find(|i| {
            self.query("sprag_palette", &format!("row.{i}"))
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref()
                == Some(title)
        })
    }
}

impl Drop for Smoke {
    /// Kill both children and remove what they left. `kill` rather than a polite shutdown: the
    /// daemon deliberately outlives its clients, so asking it to leave is not something a smoke can
    /// rely on — and its state directory is this run's own, so nothing durable is lost.
    fn drop(&mut self) {
        let _ = self.gui.kill();
        let _ = self.gui.wait();
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_file(&self.gui_sock);
        let _ = std::fs::remove_file(&self.host_sock);
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

/// Spawn `binary` with the smoke's isolated environment.
///
/// Output is discarded: a daemon's tracing on stderr would bury the checks, and anything that
/// matters is observable through the RPC surface — which is the point of a scene-as-data client.
fn spawn(binary: &Path, host: &Path, gui: &Path, state: &Path) -> io::Result<Child> {
    Command::new(binary)
        .env("SPRAG_HOST_RPC_SOCK", host)
        .env("SPRAG_GUI_HOST_SOCK", host)
        .env("SPRAG_RPC_SOCK", gui)
        .env("XDG_STATE_HOME", state)
        // Mesa lavapipe: software Vulkan, so wgpu finds a device with no GPU surface (see the
        // module docs — this is the single least guessable line in the file).
        .env(
            "VK_ICD_FILENAMES",
            "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json",
        )
        .env("WGPU_BACKEND", "vulkan")
        .env("SPRAG_GUI_PANES", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

/// Wait for `path` to exist — the socket bind race between spawning a server and connecting to it.
fn wait_for_path(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + PATIENCE;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "{} never appeared",
                path.display()
            )));
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// Flatten a snapshot subtree into `out`, keyed by tag.
fn walk(node: &Value, out: &mut std::collections::HashMap<String, Painted>) {
    if let Some(tag) = node["tag"].as_str() {
        out.insert(
            tag.to_owned(),
            Painted {
                rect: rect_of(&node["rect"]),
                text: subtree_text(node),
            },
        );
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            walk(child, out);
        }
    }
}

/// A node's laid-out `(w, h)`, when the snapshot carried one.
fn rect_of(rect: &Value) -> Option<(u32, u32)> {
    Some((
        u32::try_from(rect["w"].as_u64()?).ok()?,
        u32::try_from(rect["h"].as_u64()?).ok()?,
    ))
}

/// Every painted string in `node`'s subtree (see [`Painted::text`] for why the subtree).
fn subtree_text(node: &Value) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(content) = node["content"].as_str() {
        found.push(content.to_owned());
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            found.extend(subtree_text(child));
        }
    }
    found
}

/// What the run found.
#[derive(Default)]
struct Report {
    passed: usize,
    failed: Vec<String>,
}

impl Report {
    /// Record one check, printing it as it happens so a hung run still shows how far it got.
    fn check(&mut self, what: &str, ok: bool) {
        println!("  {}  {what}", if ok { "PASS" } else { "FAIL" });
        if ok {
            self.passed += 1;
        } else {
            self.failed.push(what.to_owned());
        }
    }

    /// The summary, and the process exit code: the number of failures, so `sprag-smoke && …` works.
    fn finish(self) -> ExitCode {
        println!("\n{} passed, {} failed", self.passed, self.failed.len());
        for failure in &self.failed {
            println!("  FAILED: {failure}");
        }
        ExitCode::from(u8::try_from(self.failed.len()).unwrap_or(u8::MAX))
    }
}
