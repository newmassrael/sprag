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
//! cargo build -p sprag-gui -p sprag-host         # REBUILD FIRST: cargo test does not refresh the binaries
//! xvfb-run -a ./target/debug/sprag-smoke
//! ```
//!
//! Both packages, and not for tidiness: the smoke spawns `sprag-term` and drives it with the `sprag`
//! CLI, which live in sprag-host. A stale one of those is the failure that reports PASS against code
//! nobody just changed.
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
            check_the_sole_docked_pane_locks_its_tear_off(&mut smoke, &mut report);
            check_focus_survives_a_window_change(&mut smoke, &mut report);
            check_a_pane_can_be_created_and_closed(&mut smoke, &mut report);
            // AFTER a check that answers a confirmation, and THAT ordering is load-bearing: the
            // state a confirmed row leaves behind is the whole claim. It replaces the opposite
            // constraint this list used to carry, when every focus-needing check had to run before
            // the first confirmation because one leaked the palette's modal scope for good.
            check_a_confirmed_row_leaves_the_focus_stack_clean(&mut smoke, &mut report);
            check_a_window_closes_under_a_live_client(&mut smoke, &mut report);
            // Needs the client ALIVE — it asks the client what its own last frame cost, so it cannot
            // join the log-reading check below the session kill.
            check_the_frames_report_their_settle_work(&mut smoke, &mut report);
            // Both need the client alive AND need the run's work already done: one asserts that
            // mirrors were stored, the other that focus requests were made, and every check above
            // is what does both. Placed here rather than earlier so neither can pass vacuously.
            check_the_agent_mirror_settles_like_the_paint(&mut smoke, &mut report);
            check_sprag_focus_requests_reach_the_re_derive(&mut smoke, &mut report);
            // The last two producers with no reader. Both take DELTAS, so they must run after the
            // work above has warmed the client — a cold client's first read re-derives and its first
            // paint shapes everything, and either would report a startup cost as a steady state.
            check_an_agents_read_costs_no_scene_rederive(&mut smoke, &mut report);
            check_the_mirror_reshapes_nothing_it_has_shaped(&mut smoke, &mut report);
            // The per-frame half of the same instrument, and the only check here that drives the
            // DAEMON rather than the client: it needs a change with no scene RPC of ours in front of
            // it (see the function docs), so it must run while both are alive and the session it
            // renames a window in still exists.
            check_terminal_output_never_reaches_the_shaper(&mut smoke, &mut report);
            // AFTER it, and that ordering is load-bearing twice over: the check above needs
            // exactly ONE pane to make the daemon-to-client pane correspondence unambiguous, and
            // this one SPLITS until the pane set can attribute its own cost. It also leaves those
            // panes standing, so nothing that counts panes may follow it.
            check_the_host_projects_panes_only_for_a_grid_reader(&mut smoke, &mut report);
            // LAST over the WIRE, and it must stay last: it destroys the session this client is
            // attached to, so the client leaves and every check after it would be asserting against
            // a dead socket.
            check_killing_the_attached_session_ends_the_client(&mut smoke, &mut report);
            // After it, deliberately: this one reads the log the departed client left behind, so
            // running it here covers every frame of the whole run and needs nothing alive.
            check_every_painted_frame_settled(&smoke, &mut report);
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
    // `Kill pane` acts on the focused pane, so it is only OFFERED with one focused — and the pane
    // set has moved under the client since anything last held focus ([`Smoke::focus_pane`]).
    if let Some(&first) = smoke.docked_panes().first() {
        report.check(
            "a pane can be focused to be killed",
            smoke.focus_pane(first),
        );
    }

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

/// A row that was CONFIRMED leaves the focus stack exactly as it found it.
///
/// The palette closes itself and the confirmation opens in the same dispatch — a modal HANDOFF, two
/// stack edits from one user action. Until pinion R1456 the shell's mailbox was a single
/// last-write-wins slot, so the palette's `Close` was overwritten and its scope stayed on the stack
/// for the life of the process: from the first confirmed row on, `focus/set` was refused for every
/// pane and every pane-scoped row stopped being offered. sprag could neither work around it nor —
/// the part that made it expensive — DETECT it, because a binding can read
/// `focus_state::focused()` and nothing of the stack beneath it.
///
/// So it is measured from OUTSIDE the process, in the three shapes the leak took: the enumeration
/// the focus manager will accept, a pane accepting focus, and a pane-scoped row still being
/// offered. The third is the one a user meets, and it is why the leak went unnoticed so long — the
/// symptom does not resemble its cause, it reads as commands quietly going missing.
fn check_a_confirmed_row_leaves_the_focus_stack_clean(smoke: &mut Smoke, report: &mut Report) {
    // Read BEFORE opening anything. A leaked scope IS the active enumeration, so opening the
    // palette here would install a fresh trap over the very state under test.
    let focusables = smoke.focusables();
    report.check(
        &format!("the focus enumeration is the app's own again ({focusables:?})"),
        focusables
            .iter()
            .any(|tag| tag.starts_with("sprag_gui.pane.")),
    );

    let Some(&pane) = smoke.docked_panes().first() else {
        report.check("a docked pane to put the keyboard back on", false);
        return;
    };
    report.check(
        &format!("a pane still takes the keyboard after a confirmation (pane {pane})"),
        smoke.focus_pane(pane),
    );

    // And the symptom a user would actually report. `Kill pane` is pane-scoped, so it is offered
    // only while a pane holds the focus — under the leak it was simply absent from the catalog.
    if smoke.invoke("sprag_palette", "open", Value::Null).is_err()
        || smoke.wait_for_tag("sprag_palette_panel").is_err()
    {
        report.check("the palette re-opens after a confirmation", false);
        return;
    }
    let offered = smoke.row_titles();
    report.check(
        &format!("the palette still offers its pane-scoped rows ({offered:?})"),
        offered.iter().any(|title| title == "Kill pane"),
    );
    // Dismissed, not left standing: the next check opens the palette itself, and a surface left
    // open would have it asserting about this one's leftovers.
    let _ = smoke.invoke("sprag_palette", "send", json!("scrim:PointerUp"));
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
    // Whatever the strip holds NOW is the baseline — the focus check above deliberately leaves the
    // window it opened standing, so a hard-coded count here would be asserting the order of the
    // checks rather than anything about windows.
    let before = smoke.tabs();
    report.check(
        &format!("the strip has a tab to start from ({before:?})"),
        !before.is_empty(),
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
/// A pane that may not be torn off says so on the LIVE dock panel, and says it after boot.
///
/// sprag locks the sole docked pane's header (tmux semantics: the main window keeps at least one
/// terminal), and computes that lock per float/dock in `create_extra_externals`. The predicate had a
/// unit test the whole time. What no unit test could see is that the computed flag never REACHED the
/// panel: sprag's external tags are constant, so pinion's `reconcile_externals` took its steady-state
/// early-return and discarded the rebuilt external, leaving the boot value in force — the lock was
/// create-time-only for as long as it took PINION-PR42 to land, with a green test beside it.
///
/// So the claim under test is a TRANSITION, not a value: two docked panes are both movable, and
/// floating one must flip the survivor to non-movable ON THE LIVE EXTERNAL. Reading the flag at boot
/// would prove nothing — a create-time-only flag is correct at create time, which is exactly how this
/// hid.
///
/// Which pane ends up docked is DISCOVERED from what the main window still paints, not predicted from
/// which one the float acted on: a floated pane moves to its own OS window, so the dock membership is
/// readable, and predicting it would re-derive the very routing that could be wrong.
fn check_the_sole_docked_pane_locks_its_tear_off(smoke: &mut Smoke, report: &mut Report) {
    // The float row acts on the FOCUSED pane, and headless there is none to start with
    // ([`Smoke::focus_pane`] says why it must be driven rather than waited for).
    report.check("a pane can be focused to act on", smoke.focus_pane(0));
    if !smoke.run_palette_row("Split into a new pane", report) {
        return;
    }
    let Ok(docked) = smoke.wait_for(|s| {
        let panes = s.docked_panes();
        (panes.len() == 2).then_some(panes)
    }) else {
        report.check("a second pane docks so either one may float", false);
        return;
    };
    let movability: Vec<Option<bool>> = docked.iter().map(|&i| smoke.panel_is_movable(i)).collect();
    report.check(
        &format!("both docked panes start out movable ({movability:?})"),
        movability.iter().all(|m| *m == Some(true)),
    );

    // Float one of them. The survivor is then the last docked pane. Which pane the float ACTS on is
    // chosen here (the row acts on the focused pane); which one is left DOCKED is still read back
    // from the paint below, because that is the routing the lock is computed from.
    let focused = smoke.focus_pane(docked[1]);
    let focusables = smoke.focusables();
    report.check(
        &format!(
            "pane {} can be focused to be floated (focusable: {focusables:?})",
            docked[1]
        ),
        focused,
    );
    if !smoke.run_palette_row("Toggle floating pane", report) {
        return;
    }
    let Ok(remaining) = smoke.wait_for(|s| {
        let panes = s.docked_panes();
        (panes.len() == 1).then(|| panes[0])
    }) else {
        report.check("floating one pane leaves a single docked pane", false);
        return;
    };
    let floated = docked
        .iter()
        .copied()
        .find(|&i| i != remaining)
        .expect("two docked panes, one of which is still docked");

    // THE assertion: the flag moved after boot. A create-time-only flag answers `true` here.
    let locked = smoke
        .wait_for(|s| (s.panel_is_movable(remaining) == Some(false)).then_some(()))
        .is_ok();
    report.check(
        &format!("the sole docked pane (pane {remaining}) locks its tear-off live"),
        locked,
    );
    report.check(
        &format!("and the floated pane (pane {floated}) stays movable"),
        smoke.panel_is_movable(floated) == Some(true),
    );

    // Dock it back: the lock must LIFT as dynamically as it landed. A one-way latch would pass the
    // assertion above and still leave a pane permanently unable to move. The toggle acts on the
    // focused pane, so the FLOATED one is the one to put focus on.
    smoke.focus_pane(floated);
    if !smoke.run_palette_row("Toggle floating pane", report) {
        return;
    }
    let lifted = smoke
        .wait_for(|s| {
            (s.docked_panes().len() == 2 && s.panel_is_movable(remaining) == Some(true))
                .then_some(())
        })
        .is_ok();
    report.check("re-docking lifts the lock again", lifted);
}

/// A window change leaves a PANE holding the keyboard, instead of leaving the user with nothing to
/// type into.
///
/// The panes of a window belong to that window alone, so selecting another one replaces this
/// client's whole pane set — and pinion drops focus to `None` the moment the focused tag stops being
/// painted. Nothing on the window path asked for it back, so after a switch every keystroke went
/// nowhere until the user clicked a pane. The symptom does not name its cause: the window arrives
/// looking perfectly normal and simply does not answer the keyboard.
///
/// The ring is parked on the HIGHEST docked slot on purpose. A swap refills slots from 0, so a ring
/// left on slot 0 would land on the new window's first pane by coincidence and prove nothing; only a
/// slot the new window does not reach can tell a real re-seed from an accident.
///
/// Driven through the strip's "+" BUTTON rather than the palette's `New window` row, and that is the
/// difference between measuring this and measuring something else: a palette row closes a modal in
/// the same dispatch, and the modal's focus RESTORE (pinion re-focuses the invoker on pop) would
/// decide the outcome instead of the window op. The button is the same user gesture with none of
/// that in the way.
///
/// Both directions are asserted, because they are not the same claim: leaving a window is a swap
/// onto a NEWBORN pane set, while coming back is a swap onto one whose slots this client has
/// already used.
///
/// It leaves the window it created standing — closing it is the NEXT check's claim, not this one's
/// — which is why that check reads the strip's tab count instead of assuming one.
fn check_focus_survives_a_window_change(smoke: &mut Smoke, report: &mut Report) {
    let docked = smoke.docked_panes();
    let Some(&parked) = docked.last() else {
        report.check("a docked pane to park the focus ring on", false);
        return;
    };
    report.check(
        &format!("the ring parks on the highest docked pane (pane {parked} of {docked:?})"),
        smoke.focus_pane(parked) && parked > 0,
    );
    let home = smoke.tabs();

    report.check(
        "the strip's + button activates",
        smoke
            .invoke(NEW_WINDOW_TAG, "send", json!("KeyboardActivate"))
            .is_ok(),
    );
    let Ok(grown) = smoke.wait_for(|s| {
        let tabs = s.tabs();
        (tabs.len() > home.len()).then_some(tabs)
    }) else {
        report.check("the + button opened a window", false);
        return;
    };

    // THE assertion. Waited on rather than read once: the swap lands over several frames (the op,
    // the slot reconcile, the paint that re-enumerates), and reading between them would report a
    // transient as the verdict.
    let landed = smoke.wait_for(|s| {
        let focused = s.focused()?;
        let index: usize = focused.strip_prefix("sprag_gui.pane.")?.parse().ok()?;
        s.docked_panes().contains(&index).then_some(focused)
    });
    report.check(
        &format!("a live pane still holds the keyboard in the new window ({landed:?})"),
        landed.is_ok(),
    );

    // ...and coming back. The home tab is found by NAME in the grown strip, so this selects the
    // window it means rather than a position that moved when the new tab appeared.
    let Some(at) = grown.iter().position(|name| home.first() == Some(name)) else {
        report.check("the home window still has a tab to come back to", false);
        return;
    };
    report.check(
        &format!("the home tab activates ({at})"),
        smoke
            .invoke(
                &format!("sprag_gui.wtab.{at}"),
                "send",
                json!("KeyboardActivate"),
            )
            .is_ok(),
    );
    let back = smoke.wait_for(|s| {
        let focused = s.focused()?;
        let index: usize = focused.strip_prefix("sprag_gui.pane.")?.parse().ok()?;
        (s.docked_panes().contains(&index) && s.docked_panes().len() == docked.len())
            .then_some(focused)
    });
    report.check(
        &format!("and coming home leaves a live pane holding it too ({back:?})"),
        back.is_ok(),
    );

    // The same window change, driven from the PALETTE — the other real user path, and the one with
    // a modal in it. A palette row closes its dialog in the same dispatch as the command, so two
    // things race for the ring: pinion's modal pop, which RESTORES the tag focused when the palette
    // opened, and the window op's own request. pinion R1462 settled the race in the request's favour
    // (the modal batch applies first, the request last) — but only for an op that actually makes
    // one, and this leg is what caught sprag not making it: the op saw a ring on the closing
    // palette's field, read it as "a live widget holds the caret", and asked for nothing. Asserted
    // rather than reasoned about, because both halves of that — what the shell does with two
    // requests, and whether sprag files the second — read as certain and were not.
    report.check(
        &format!("the ring parks on pane {parked} again for the palette path"),
        smoke.focus_pane(parked),
    );
    if !smoke.run_palette_row("New window", report) {
        return;
    }
    let after = smoke.wait_for(|s| {
        let focused = s.focused()?;
        let index: usize = focused.strip_prefix("sprag_gui.pane.")?.parse().ok()?;
        s.docked_panes().contains(&index).then_some(focused)
    });
    report.check(
        &format!("a live pane holds the keyboard after a PALETTE window change ({after:?})"),
        after.is_ok(),
    );
}

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

/// The settle verdict as a NUMBER the client will hand over, not as a warning it failed to print.
///
/// pinion R1459 puts `settle_passes` + `settled` on `scene/frame_timings`, which is the half
/// [`check_every_painted_frame_settled`] could not have: that one reads a diagnostic, so it can only
/// ever report an ABSENCE, and an absence is the same shape whether the frames converged or the
/// warning never had a chance to fire. This asks the client directly and gets a count back.
///
/// The two are kept side by side because neither covers the other. The log check spans every frame
/// of the whole run and answers "did any frame give up"; this one spans the LAST frame only and
/// answers "what did a frame actually cost" — a positive fact, non-vacuous by construction, since a
/// number either arrives or the check fails.
///
/// `settle_passes` is asserted against a RANGE rather than the `1` sprag measures today. One pass is
/// what a binding whose view and layout agree spends, and sprag does agree — but a second pass is a
/// legitimate frame, not a defect (a pane-viewport publish that moves a rect the next pass reads
/// back), and pinning the literal would turn a correct frame into a red smoke. What is NOT
/// legitimate is exhausting the budget, and `settled` is the field that says so: a frame that
/// converges exactly on the budget and one that gave up report the same count, so both are read.
///
/// The budget is spelled here rather than imported for the reason the paint constants above are —
/// and for one more: `pinion-runtime` is a DEV-dependency of `sprag-gui`, which a bin cannot reach.
fn check_the_frames_report_their_settle_work(smoke: &mut Smoke, report: &mut Report) {
    /// pinion's `SETTLE_PASS_BUDGET` — the passes a paint may spend before it gives up.
    const SETTLE_PASS_BUDGET: u64 = 4;

    let timings = match smoke.call("scene/frame_timings", json!({})) {
        Ok(value) => value,
        Err(error) => {
            report.check(
                &format!("the client reports its frame work ({error})"),
                false,
            );
            return;
        }
    };
    // Non-vacuity first, as next door: a settle verdict about zero frames is not evidence.
    let frames = timings["frame_count"].as_u64().unwrap_or(0);
    report.check(
        &format!("the client has painted frames to report on ({frames})"),
        frames > 0,
    );
    let passes = timings["last"]["settle_passes"].as_u64();
    let settled = timings["last"]["settled"].as_bool();
    report.check(
        &format!("the last frame settled inside the pass budget (passes: {passes:?}, settled: {settled:?})"),
        passes.is_some_and(|p| (1..=SETTLE_PASS_BUDGET).contains(&p)) && settled == Some(true),
    );
}

/// The scene an AGENT reads settled, the same way a painted frame does.
///
/// pinion R1465: a stored paint scene ran ONE pass and left its dirty bit unread, so the mirror an
/// out-of-process reader gets could be a scene a further pass would still have changed — while the
/// pixels a human saw were settled. sprag is the reason that matters: `scene/snapshot` is how the
/// CLI, the MCP surface and this smoke see anything at all, so an unsettled mirror is not a
/// cosmetic difference for us, it is every assertion in this file reading a scene the client had
/// not finished deriving.
///
/// R1465 prices it (`mirror.*`) instead of leaving it a claim, and this is the reader. The order
/// matters as much as the assertion: `scenes_total` is checked FIRST because `unsettled_total == 0`
/// is trivially true when no mirror was ever stored, and a smoke that passes by not doing the work
/// is the failure mode this project keeps re-learning. By the time this runs, every check above has
/// taken snapshots, so a zero here would itself be the bug.
fn check_the_agent_mirror_settles_like_the_paint(smoke: &mut Smoke, report: &mut Report) {
    let timings = match smoke.call("scene/frame_timings", json!({})) {
        Ok(value) => value,
        Err(error) => {
            report.check(
                &format!("the client prices its mirror work ({error})"),
                false,
            );
            return;
        }
    };
    let mirror = &timings["mirror"];
    let scenes = mirror["scenes_total"].as_u64().unwrap_or(0);
    let passes = mirror["passes_total"].as_u64();
    let unsettled = mirror["unsettled_total"].as_u64();
    report.check(
        &format!(
            "the agent-facing mirror was actually stored ({scenes} scenes, {passes:?} passes)"
        ),
        scenes > 0,
    );
    report.check(
        &format!("every scene an agent read had settled ({unsettled:?} unsettled)"),
        unsettled == Some(0),
    );
}

/// sprag's focus REQUESTS reach pinion's re-derive, as a count rather than an inference.
///
/// This is the number sprag's R1462 round had to dig out with a temporary `eprintln` in
/// `reseed_pane_focus`, because a binding can see that it asked for focus and cannot see whether
/// anything downstream enumerated the windows to honour it. pinion R1464 puts the enumeration's own
/// total on the wire (R1463 having made the re-derive span every painted window), so the question is
/// answerable from out here now.
///
/// Asserted as PRESENCE, not as a budget. What sprag needs to know is that its requests are not
/// falling into a path that never runs — the R1462 failure mode exactly, where the palette leg made
/// no request at all and every downstream fix was therefore irrelevant. A specific count would be
/// pinning pinion's internals, which is not sprag's to fix if it moves.
fn check_sprag_focus_requests_reach_the_re_derive(smoke: &mut Smoke, report: &mut Report) {
    let timings = match smoke.call("scene/frame_timings", json!({})) {
        Ok(value) => value,
        Err(error) => {
            report.check(
                &format!("the client reports its focus work ({error})"),
                false,
            );
            return;
        }
    };
    let derivations = timings["focus"]["derivations_total"].as_u64();
    let retries = timings["focus"]["retries_total"].as_u64();
    report.check(
        &format!("sprag's focus requests reached the re-derive ({derivations:?} derivations, {retries:?} retries)"),
        derivations.is_some_and(|total| total > 0),
    );
}

/// One `scene/frame_timings` sample, with a failed call reported rather than swallowed.
///
/// The two producer-work checks below each read the reply three times or more to take a DELTA, which
/// is what upstream says these cumulative totals are for — so they need the sample as a value, not
/// as the once-per-check inline match the older readers use.
fn frame_work(smoke: &mut Smoke, what: &str, report: &mut Report) -> Option<Value> {
    match smoke.call("scene/frame_timings", json!({})) {
        Ok(value) => Some(value),
        Err(error) => {
            report.check(&format!("{what} ({error})"), false);
            None
        }
    }
}

/// An AGENT's read of this client re-derives no scene.
///
/// `scene/snapshot` is the one call sprag's whole agent surface stands on — the CLI, the `sprag-mcp`
/// tools an AI in a pane drives its siblings with, and every assertion in this file. pinion R1460
/// prices it: a read served from the committed frame never reaches the RPC scene producer, while one
/// that cannot be served runs a full view + layout settle. Which of those sprag pays, per call, was
/// unanswerable from out here before the counter existed and is a real number for a client an agent
/// polls in a loop.
///
/// **The detector is asserted BEFORE the claim**, because "the delta is zero" is also what a dead
/// counter says, and a check that cannot fail is the failure mode this project keeps re-learning. A
/// path-addressed call is the driver: resolving a tag to a point needs a scene laid out at the live
/// viewport, which the stored one cannot answer for, so it must produce. Measured +1 per call against
/// a snapshot's +0, and `scene/layout` is NOT an alternative — it reads as a re-derive and measured
/// zero, so it would have proved nothing.
///
/// The click that drives it is inert beyond the resolve (synthetic pointer input does not drain
/// headless), but this check does not rest on that: it reads counters only, and pane 0 already holds
/// the ring, so a landing click would change nothing it or anything after it asserts.
fn check_an_agents_read_costs_no_scene_rederive(smoke: &mut Smoke, report: &mut Report) {
    let Some(start) = frame_work(smoke, "the client prices its producer work", report) else {
        return;
    };
    let idle = start["produce"]["passes_total"].as_u64();

    let resolved = smoke.call("scene/click", json!({ "path": "sprag_gui.pane.0" }));
    let Some(driven) = frame_work(smoke, "the client re-prices after a re-derive", report) else {
        return;
    };
    let derived = driven["produce"]["passes_total"].as_u64();
    report.check(
        &format!(
            "a path-addressed call DOES re-derive the scene ({idle:?} -> {derived:?}, {resolved:?})"
        ),
        matches!((idle, derived), (Some(before), Some(after)) if after > before),
    );

    // The claim, now that a zero can be told from a counter that never moves.
    let _ = smoke.call(
        "scene/snapshot",
        json!({ "path": "/window[main]", "from": "paint" }),
    );
    let Some(read) = frame_work(smoke, "the client re-prices after a read", report) else {
        return;
    };
    let after_read = read["produce"]["passes_total"].as_u64();
    report.check(
        &format!("an agent's read re-derives NOTHING ({derived:?} -> {after_read:?})"),
        after_read.is_some() && after_read == derived,
    );
}

/// sprag's re-store fan-out hands the shaper no text it has already shaped.
///
/// A `shape_miss` is a `LayoutCache` MISS — a text run handed to the shaper — and pinion R1454
/// measured one at 18.5us against a 118ns hit, so a pass that re-shapes its whole working set every
/// time is a third of a 60fps frame at 300 strings. R1454 bounded the worst offender, but upstream is
/// explicit that the bound is **consumer-honoured**: *"a binding that ignores it still measures every
/// row, and nothing noticed. This is what notices."* sprag is a consumer that pays this on every
/// MUTATING call, not merely per frame — each one leaves a stored mirror behind for `from: paint`,
/// one side-effect-free view + layout per painted window.
///
/// What this check DEMONSTRATES it prices is sprag's CHROME: the detector moves the counter by
/// writing a field, and a field is chrome. Whether pane CONTENT reaches the shaper is a separate
/// question and nothing here answers it — the pointers say no (two new windows, each with a fresh
/// grid, moved this number by zero, and pinion sizes a grid off a `CellMetric` lattice rather than
/// shaping rows), but no check drives novel text into a pane, so the grid is unproven either way.
/// Recorded because "a terminal's biggest text surface is its cells" is the natural assumption, and a
/// future round reading a green tick here as coverage of pane content would be reading in a claim
/// that was never made.
///
/// Detector first again, and it is the harder half: every steady-state number in this run is zero, so
/// the only way to know the counter is alive is to make it move. Novel text into a painted field does
/// it (+2 per string, measured), and the miss is accounted SYNCHRONOUSLY in the dispatch — no sleep,
/// no wait, so the check has no timing in it at all.
fn check_the_mirror_reshapes_nothing_it_has_shaped(smoke: &mut Smoke, report: &mut Report) {
    // The detector writes into the find bar, which an earlier check opened. Asserted rather than
    // assumed: with the field unpainted the write would change no painted text, the counter would sit
    // still, and the detector would read as broken when it was only unreachable.
    report.check(
        "the find field is painted to drive novel text through",
        smoke.tags().contains_key("sprag_find"),
    );
    let Some(start) = frame_work(smoke, "the client prices its mirror shaping", report) else {
        return;
    };
    let cold = start["mirror"]["shape_misses_total"].as_u64();

    // Text no shaper on this machine has seen: two scripts and a nonsense run, so it cannot collide
    // with a label the UI already painted.
    let wrote = smoke.call(
        "scene/intervene",
        json!({
            "path": "/sprag_find/external/text",
            "value": "zqxjvwkbp \u{4e2d}\u{6587}\u{6d4b}\u{8bd5} \u{0416}\u{0439}\u{0446}",
        }),
    );
    let Some(shaped) = frame_work(smoke, "the client re-prices after novel text", report) else {
        return;
    };
    let warm = shaped["mirror"]["shape_misses_total"].as_u64();
    report.check(
        &format!("novel text DOES reach the shaper ({cold:?} -> {warm:?}, {wrote:?})"),
        matches!((cold, warm), (Some(before), Some(after)) if after > before),
    );

    // The claim. A mutating call with nothing new on screen must store its mirrors for free.
    let scenes_before = shaped["mirror"]["scenes_total"].as_u64();
    let _ = smoke.call("scene/click", json!({ "path": "sprag_gui.pane.0" }));
    let Some(steady) = frame_work(smoke, "the client re-prices a steady-state store", report)
    else {
        return;
    };
    let scenes_after = steady["mirror"]["scenes_total"].as_u64();
    let misses_after = steady["mirror"]["shape_misses_total"].as_u64();
    // Non-vacuity before the verdict, exactly as next door: no mirror stored means no shaping to
    // account, and a zero delta would then be true of a call that did nothing at all.
    report.check(
        &format!(
            "a mirror was actually re-stored to price ({scenes_before:?} -> {scenes_after:?})"
        ),
        matches!((scenes_before, scenes_after), (Some(before), Some(after)) if after > before),
    );
    report.check(
        &format!("and it re-shaped nothing ({warm:?} -> {misses_after:?})"),
        misses_after.is_some() && misses_after == warm,
    );
}

/// Novel terminal OUTPUT reaches a user's pixels without costing pinion's shaper a single run.
///
/// This is the per-frame half of the shaping instrument and the question R215 left open. The
/// cumulative counter next door demonstrably prices sprag's CHROME — its detector writes a text
/// FIELD, and a field is chrome — while "a terminal's biggest text surface is its cells" stayed an
/// assumption nothing had tested. `last.shape_misses` is the field that can answer it, because the
/// grid is the one surface that repaints with no scene RPC of ours in front of it.
///
/// That ordering is why this check is shaped the way it is, and it was MEASURED before a line of it
/// was written: a mutating scene RPC stores its mirror synchronously in the dispatch, and that store
/// walks the same text and warms the same `LayoutCache` — so text written over RPC is already shaped
/// by the time a frame paints it, and the frame reports zero. Driving this from the scene socket
/// would have produced a green tick that priced the mirror rather than the paint. The driver has to
/// reach the DAEMON and let the client find out on its own poll, which is why the CLI and a second
/// connection appear in this check and nowhere else in this file.
///
/// The detector is asserted before the claim, and proves three things at once: a window renamed over
/// the CLI paints a novel tab, the frame that paints it reports misses where the steady state
/// reports none — so the field is live, chrome text DOES reach the shaper, and nothing on this path
/// pre-warmed it. Without it a green claim would be indistinguishable from a counter that never
/// moves, which is the failure mode this project keeps re-learning.
///
/// What the claim does NOT say is that the cells are free. sprag paints its grid from the rows the
/// host serialises rather than from text nodes pinion lays out, so this instrument cannot see
/// whatever that path costs. It says that terminal output does not scale PINION's shaper — the cost
/// pinion R1454 measured at 18.5us a miss against a 118ns hit, which is the reason the question was
/// worth answering at all.
///
/// The frames are SAMPLED, because `last` is the last frame and no cumulative per-paint counter
/// exists. `contiguous` is what keeps that honest: a frame number that jumps means one slipped
/// between two samples, and "not one frame shaped" would then be a claim about a frame never read.
fn check_terminal_output_never_reaches_the_shaper(smoke: &mut Smoke, report: &mut Report) {
    /// A window name no shaper on this machine has seen, in three scripts so it cannot collide with
    /// anything the UI already paints.
    const NOVEL_WINDOW: &str = "zqxjvw\u{03a9}\u{4e00}\u{4e8c}\u{4e09}";
    /// How many novel lines the pane is made to print. More than one, because a single line could
    /// be shaped in a frame the sampler happened to be between.
    const LINES: usize = 12;

    let Some(session) = smoke.attached_session() else {
        report.check("the client says which session to drive", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the daemon takes a second connection to drive it by", false);
        return;
    };
    // Both sides are asked, and both must answer ONE — the daemon addresses a pane by id and the
    // client paints it by index, and nothing on the wire maps one to the other. With a single pane
    // on each side the correspondence is not a guess; with more it would be, and this says so
    // instead of driving whichever pane the ids happen to favour.
    let ids = daemon_panes(&mut daemon, &session);
    let painted = smoke.docked_panes();
    report.check(
        &format!(
            "the daemon and the client agree on ONE pane to drive (daemon {ids:?}, painted {painted:?})"
        ),
        matches!((ids.as_slice(), painted.as_slice()), ([_], [_])),
    );
    // Nothing below may run on a guess: with the correspondence unproven, driving whichever pane the
    // ids happen to favour would let the claim pass or fail on which pane got the text.
    let ([id], [index]) = (ids.as_slice(), painted.as_slice()) else {
        return;
    };
    let (id, index) = (*id, *index);

    // ── The detector: novel CHROME text, over the same host path, before anything is claimed.
    let from = smoke.frame_count();
    let renamed = smoke.cli(&["rename-window", NOVEL_WINDOW, "-t", &session]);
    let watch = smoke.watch_frames(from, |s| s.tabs().iter().any(|name| name == NOVEL_WINDOW));
    report.check(
        &format!("a host-driven rename reaches the client's painted strip ({renamed:?})"),
        watch.arrived,
    );
    report.check(
        &format!(
            "novel CHROME text DOES reach the shaper ({:?})",
            watch.misses()
        ),
        watch.shaped(),
    );

    // ── The claim: novel OUTPUT, arriving the one way nothing of ours precedes.
    //
    // One line per drive rather than one burst of twelve, because a burst lands in a single frame
    // under a software rasteriser and a claim about "every frame" would then rest on one. Twelve
    // drives are twelve independent frames, each painting text no shaper has been handed before.
    let mut watch = FrameWatch::span();
    let mut printed = Ok(Value::Null);
    for line in (0..LINES).map(pane_line) {
        let from = smoke.frame_count();
        printed = daemon.call(
            "scene/invoke",
            json!({
                "path": format!("/pane_{id}/sprag_input/external/text"),
                "args": { "text": format!("echo {line}\n") },
                "session": session,
            }),
        );
        watch.absorb(smoke.watch_frames(from, |s| {
            s.pane_rows(index).iter().any(|row| row.contains(&line))
        }));
        if !watch.arrived {
            break;
        }
    }
    // Non-vacuity, and it comes first: frames that shaped nothing while nothing arrived are not
    // evidence about a grid, they are evidence that the pane never printed.
    report.check(
        &format!("the novel output reached the PAINTED grid ({printed:?})"),
        watch.arrived,
    );
    report.check(
        &format!(
            "and every frame it took was seen ({} frames, contiguous: {})",
            watch.frames.len(),
            watch.contiguous
        ),
        !watch.frames.is_empty() && watch.contiguous,
    );
    report.check(
        &format!(
            "not one of them handed the shaper a run ({:?})",
            watch.misses()
        ),
        watch.frames.iter().all(|(_, misses)| *misses == Some(0)),
    );
}

/// A request pays the grid only when it can READ the grid — over a real socket, from a real client.
///
/// R217 built `sprag_grid::work` and found that ANY call on the daemon's socket re-projected every
/// pane's entire grid, silent panes included: `scene/revision`, which mutates nothing and reports
/// one integer, cost the same whole-screen walk per pane that a snapshot did. This check was
/// written then, and it asserted exactly that — twenty spaced reads, twenty-one pane sets.
///
/// R218's projection gate (`sprag_host::rpc::pane_cells_for`) removed the cost, so the checks below
/// now assert its ABSENCE. That inversion is the point: they were true measurements of a real
/// defect, and the way a measurement earns its keep is by turning red when the defect goes. What
/// changed is the host, not the instrument.
///
/// Two halves, and neither stands alone:
///
/// * **The claim** — a window of pure reads moves the meter by nothing at all. Zero is a strong
///   assertion here rather than a weak one, because the same window used to cost a pane set per
///   call, and because the client is attached and parked throughout: a stray wake would fetch every
///   pane and show up immediately.
/// * **The positive control** — `scene/snapshot`, the one method that genuinely reads every pane's
///   grid, must still pay for every pane's grid. Without it the zero above would also be reported
///   by a meter that had simply stopped counting, or by a host that had stopped projecting at all.
///
/// The control keeps R217's DIVISIBILITY argument, which is how an aggregate counter attributes
/// work it does not label: each projection adds one pane's whole area, so a host that projected
/// only the pane a request named would leave a total that is a multiple of THAT pane's area and
/// never of the whole set's. **The arithmetic only separates those two worlds when the set is
/// ASYMMETRIC**, so this splits until it is and then asserts that it is, rather than assuming a
/// layout — two equal panes make "both projected once" and "one projected twice" the same number.
///
/// One R217 bound is GONE and worth recording: the instrument no longer perturbs what it measures.
/// Reading the meter is a `scene/query`, which after the gate projects nothing, so these numbers
/// are exact rather than exact-modulo-a-pane-set.
fn check_the_host_projects_panes_only_for_a_grid_reader(smoke: &mut Smoke, report: &mut Report) {
    /// How many times to split looking for a set whose areas can attribute their own work.
    const SPLITS: usize = 3;
    /// Reads to price. Spaced, so each one's fan-out lands before the next is sent.
    const READS: u64 = 8;

    let Some(session) = smoke.attached_session() else {
        report.check("the client says which session to price", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the daemon takes a connection to read its meter", false);
        return;
    };
    let Some(&first) = daemon_panes(&mut daemon, &session).first() else {
        report.check("there is a pane to name as the driven one", false);
        return;
    };
    let named = u64::from(first);

    // Split until the set can attribute its own cost — see the docs for why a symmetric set
    // cannot. Reported below rather than trusted, so a layout change that breaks it says so.
    let mut areas = settled_pane_areas(smoke, &mut daemon, &session);
    for _ in 0..SPLITS {
        if attributable(&areas, named) {
            break;
        }
        if !smoke.run_palette_row("Split into a new pane", report) {
            break;
        }
        let panes = areas.len() + 1;
        let _ = smoke.wait_for(|s| (s.pane_count() >= panes).then_some(()));
        areas = settled_pane_areas(smoke, &mut daemon, &session);
    }
    let total: u64 = areas.values().sum();
    let one = areas.get(&named).copied().unwrap_or_default();
    report.check(
        &format!(
            "the pane set can attribute its own work (pane_{named} is {one} of {total} cells over {} panes)",
            areas.len()
        ),
        attributable(&areas, named),
    );
    if !attributable(&areas, named) {
        return;
    }

    // Let the split's own resize churn end before the window opens. It is the ONE stretch whose
    // projections are not whole sets — panes are changing size, so a buffer built mid-resize
    // belongs to an area no pane has any more, and a window containing one is offset by a
    // non-multiple for good. Waited out on the CLIENT's revision, which is the one signal here
    // that costs nothing to read.
    let mut previous = None;
    let quiet = smoke.wait_for(|s| {
        let now = s.call("scene/revision", json!({})).ok()?;
        let still = previous.as_ref() == Some(&now);
        previous = Some(now.clone());
        still.then_some(())
    });
    report.check("the client settles after the splits", quiet.is_ok());

    // ── The window: nothing but reads. No keystroke, no output, no resize.
    let Some(before) = grid_work(&mut daemon, &session) else {
        report.check("the host reports what it has projected", false);
        return;
    };
    for _ in 0..READS {
        let _ = daemon.call("scene/revision", json!({ "session": session }));
        std::thread::sleep(POLL);
    }
    let Some(after) = grid_work(&mut daemon, &session) else {
        report.check("the host still reports what it has projected", false);
        return;
    };
    let (projections, cells) = (after.0 - before.0, after.1 - before.1);

    // The geometry must not have moved under the measurement, or the areas the arithmetic rests on
    // describe a set that no longer exists. Asserted, not hoped for.
    report.check(
        &format!(
            "the pane geometry held still while it was priced ({total} cells over {} panes)",
            areas.len()
        ),
        pane_areas(&mut daemon, &session) == areas,
    );
    // THE claim: a read that cannot reach a grid does not pay for one. This used to be one whole
    // pane set per call — `{READS} reads` cost `{READS}` sets — and is now nothing whatsoever.
    report.check(
        &format!(
            "{READS} reads of a NUMBER cost the grid nothing ({projections} projections, {cells} cells)"
        ),
        projections == 0 && cells == 0,
    );

    // The positive control, without which the zero above is equally well explained by a meter that
    // stopped counting. The ONE method that reads every pane's grid must still pay for every pane's
    // grid — measured on the same connection, in the same window, against the same set.
    let Some(before) = grid_work(&mut daemon, &session) else {
        report.check("the host reports its meter before the snapshot", false);
        return;
    };
    let snapped = daemon
        .call("scene/snapshot", json!({ "path": "", "session": session }))
        .is_ok();
    let Some(after) = grid_work(&mut daemon, &session) else {
        report.check("the host reports its meter after the snapshot", false);
        return;
    };
    let (projections, cells) = (after.0 - before.0, after.1 - before.1);
    report.check("the daemon answered a snapshot to price it", snapped);
    // Exact, because nothing here perturbs any more; and divisible by the SET while not by the
    // named pane, which is the half that rules out "one pane projected several times".
    report.check(
        &format!(
            "a snapshot still projects every pane, whole ({projections} projections, {cells} cells = the {total}-cell set, not a multiple of pane_{named}'s {one})"
        ),
        projections == areas.len() as u64
            && cells == total
            && !cells.is_multiple_of(one),
    );
}

/// Whether `areas` can say which panes were projected, given the one a request `named`.
///
/// True exactly when the set's total is NOT a multiple of that pane's own area: only then does "a
/// multiple of the set" rule out "a multiple of the named pane". A single pane is never
/// attributable, and neither are two equal ones.
fn attributable(areas: &std::collections::HashMap<u64, u64>, named: u64) -> bool {
    let total: u64 = areas.values().sum();
    let one = areas.get(&named).copied().unwrap_or_default();
    one > 0 && !total.is_multiple_of(one)
}

/// Each pane's cell area, by pane id, off the host's own pane list.
///
/// Read from the wire rather than computed from the client's tiles: the host projects what the
/// HOST thinks a pane measures, and the arithmetic here has to use the same number.
fn pane_areas(daemon: &mut HostConn, session: &str) -> std::collections::HashMap<u64, u64> {
    let mut areas = std::collections::HashMap::new();
    if let Ok(list) = daemon.call(
        "scene/query",
        json!({ "path": "/sprag_mux/external/panes", "session": session }),
    ) {
        for pane in list.as_array().into_iter().flatten() {
            if let (Some(id), Some(cols), Some(rows)) = (
                pane["id"].as_u64(),
                pane["cols"].as_u64(),
                pane["rows"].as_u64(),
            ) {
                areas.insert(id, cols * rows);
            }
        }
    }
    areas
}

/// Each pane's area, once the layout has stopped moving.
///
/// A split resizes, and areas read mid-resize describe a set that will not exist by the time
/// anything is measured against it.
fn settled_pane_areas(
    smoke: &mut Smoke,
    daemon: &mut HostConn,
    session: &str,
) -> std::collections::HashMap<u64, u64> {
    let mut previous = None;
    smoke
        .wait_for(|_| {
            let now = pane_areas(daemon, session);
            let still = !now.is_empty() && previous.as_ref() == Some(&now);
            previous = Some(now.clone());
            still.then_some(now)
        })
        .unwrap_or_default()
}

/// The host's projection meter — `(projections_total, cells_total)`, both monotonic since boot.
fn grid_work(daemon: &mut HostConn, session: &str) -> Option<(u64, u64)> {
    let value = daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/grid_work", "session": session }),
        )
        .ok()?;
    Some((
        value["projections_total"].as_u64()?,
        value["cells_total"].as_u64()?,
    ))
}

/// One line of the novel output, `i` distinguishing it from its siblings.
///
/// Takes the index as a DISPLAY so the same function spells both the `printf` format the pane runs
/// and the needle the assertion looks for — two spellings of one string is how a check ends up
/// waiting for text it never asked for.
fn pane_line(i: impl std::fmt::Display) -> String {
    format!("zqxjvw\u{03a8}{i}\u{4e03}\u{516b}\u{4e5d}")
}

/// Which panes the DAEMON says `session`'s current window holds, by id.
///
/// Asked of the daemon's own scene rather than derived from the client's tile indices: the ids are
/// minted host-side and the client never paints them, so any mapping computed out here would be a
/// guess dressed as an address.
fn daemon_panes(daemon: &mut HostConn, session: &str) -> Vec<u32> {
    let Ok(tree) = daemon.call("scene/snapshot", json!({ "path": "", "session": session })) else {
        return Vec::new();
    };
    let mut tags = Vec::new();
    collect_tags(&tree, &mut tags);
    let mut ids: Vec<u32> = tags
        .iter()
        .filter_map(|tag| tag.strip_prefix("pane_")?.parse().ok())
        .collect();
    ids.sort_unstable();
    ids
}

/// Every tag in a scene tree, in document order.
fn collect_tags(node: &Value, out: &mut Vec<String>) {
    if let Some(tag) = node["tag"].as_str() {
        out.push(tag.to_owned());
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            collect_tags(child, out);
        }
    }
}

/// Every frame this client painted reached a FIXED POINT before it was presented.
///
/// pinion R1458 re-runs `view` + layout until a pass moves nothing, because a layout pass writes
/// state the view reads back — a scroll bound, a pane's measured rect — so the scene a pass just
/// laid out can already be stale, and the honest one to present is the scene a pass no longer
/// changes. A binding whose two sides disagree about a value each derives from the other converges
/// never; the shell then paints the last pass it has, requests another frame, and WARNS.
///
/// sprag is exactly the binding that could do that: the pane-viewport publish drives a PTY resize
/// whose reflow changes the grid the next pass lays out, and `reconcile_frame` grows each pane's
/// scroll bound from an off-thread producer. So "sprag's frames settle" is a claim about SPRAG, and
/// nothing else in this repo makes it.
///
/// It is read from the client's LOG, and that is still the right channel for this claim even though
/// pinion R1459 has since put the verdict on the wire as well. The wire answers for the last frame
/// only ([`check_the_frames_report_their_settle_work`] asks it); the diagnostic is the only witness
/// to every OTHER frame, including the ones painted while nothing was polling. A run's worth of
/// frames and the most recent frame are different claims, so both are made.
fn check_every_painted_frame_settled(smoke: &Smoke, report: &mut Report) {
    let log = smoke.gui_log();
    // Non-vacuity, and it comes FIRST: an absent warning is evidence only if a present one would
    // have arrived. A check that reads a channel nothing could ever reach passes forever and means
    // nothing, so the channel is asserted before what it carries.
    let lines = log.lines().count();
    report.check(
        &format!("the client's own diagnostics reached the smoke ({lines} lines)"),
        lines > 0,
    );
    let unsettled: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("did not settle"))
        .collect();
    report.check(
        &format!(
            "every painted frame settled within the pass budget ({} unsettled)",
            unsettled.len()
        ),
        unsettled.is_empty(),
    );
    for line in unsettled.iter().take(3) {
        println!("        {line}");
    }
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
/// The window strip's "+" (new window) button tag — the same gesture a user clicks, addressed
/// symbolically because a synthesised pointer coordinate never lands headless.
const NEW_WINDOW_TAG: &str = "sprag_gui.wnew";

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
    /// What a terminal GRID in this node's subtree is showing, one string per row.
    ///
    /// Separate from [`Self::text`] because it arrives by a different route entirely: a pane's cells
    /// are not text nodes pinion laid out, they are rows the host serialised and the grid paints
    /// itself. Reading them is how a check can tell "the output reached the pixels" from "the pane
    /// never printed", which nothing in this file could do before.
    rows: Vec<String>,
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
    /// Where this run's binaries live — the daemon and client it spawned, and the CLI it drives the
    /// daemon with. Taken from the smoke's OWN path so a run always drives the build it came from.
    target: PathBuf,
    /// Everything the GUI wrote to stderr for the whole run — the diagnostics it emits about
    /// ITSELF, which no scene query can answer. Lives under [`Self::state`], so the teardown that
    /// removes the run's directory takes it too; read it before the `Smoke` drops.
    gui_log: PathBuf,
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

        let daemon_log = state.join("daemon.log");
        let gui_log = state.join("gui.log");

        let daemon = spawn(
            &target.join("sprag-term"),
            &host_sock,
            &gui_sock,
            &state,
            &daemon_log,
        )?;
        wait_for_path(&host_sock)?;
        let gui = spawn(
            &target.join("sprag-gui"),
            &host_sock,
            &gui_sock,
            &state,
            &gui_log,
        )?;
        wait_for_path(&gui_sock)?;
        let conn = HostConn::connect(&gui_sock, PATIENCE)?;

        let mut smoke = Self {
            daemon,
            gui,
            conn,
            gui_sock,
            host_sock,
            state,
            target,
            gui_log,
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

    /// Which pane tiles the MAIN window is painting, by index and in order.
    ///
    /// Reads the pane's own tag and not its `#grid` child: a pane paints several tagged nodes under
    /// one composite prefix, so a naive prefix count moves by more than one per pane and would make
    /// "one more pane" unstateable.
    ///
    /// Main-window-scoped, which is what makes it the DOCKED set: a floated pane moves to its own
    /// `pane-{i}` OS window and stops painting here, so this answers the dock membership the
    /// tear-off lock is computed from — without asking the client for the very fact under test.
    fn docked_panes(&mut self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .tags()
            .keys()
            .filter_map(|tag| tag.strip_prefix("sprag_gui.pane.")?.parse().ok())
            .collect();
        indices.sort_unstable();
        indices
    }

    /// How many pane tiles the main window is painting.
    fn pane_count(&mut self) -> usize {
        self.docked_panes().len()
    }

    /// Put the within-app focus on pane `i`, so the palette's pane-scoped rows are offered.
    ///
    /// Driven rather than assumed, for the reason the boot check already records: a `focus_request`
    /// needs a winit input tick to drain, and there is none headless — so any focus sprag ASKS for
    /// (at boot, or on a window change) never arrives here. A check that needs a focused pane must
    /// therefore set one, and that is setup, not the claim under test.
    /// Verified by reading focus back: `focus/set` answers `Ok` for a tag the focus manager will
    /// not actually hold (one outside the active enumeration), so the call's own result is not
    /// evidence that anything moved.
    fn focus_pane(&mut self, i: usize) -> bool {
        let tag = format!("sprag_gui.pane.{i}");
        let _ = self.call("focus/set", json!({ "tag": tag }));
        self.focused().as_deref() == Some(tag.as_str())
    }

    /// The tag holding the within-app focus, if any.
    fn focused(&mut self) -> Option<String> {
        self.call("focus/get", json!({})).ok()?["focused"]
            .as_str()
            .map(str::to_owned)
    }

    /// Every tag the focus manager will accept, in Tab order — what a refused `focus/set` was
    /// measured against.
    fn focusables(&mut self) -> Vec<String> {
        self.call("focus/get", json!({}))
            .ok()
            .and_then(|value| {
                Some(
                    value["tab_order"]
                        .as_array()?
                        .iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect(),
                )
            })
            .unwrap_or_default()
    }

    /// Whether pane `i`'s LIVE dock panel says its header may start a drag.
    ///
    /// Read off the external pinion actually holds, not off sprag's predicate: the predicate was
    /// right for a long time while the live panel kept its stale boot value, and only this address
    /// can tell those apart.
    fn panel_is_movable(&mut self, i: usize) -> Option<bool> {
        self.query(&format!("terminal-{i}"), "movable")
            .ok()?
            .as_bool()
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
            // Say what WAS offered. A row can go missing because its command was withdrawn, because
            // the catalog froze in a state that gates it out, or because the title moved — and those
            // read identically as a bare "not offered".
            let offered = self.row_titles();
            report.check(
                &format!("the palette offers `{title}` (offered: {offered:?})"),
                false,
            );
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

    /// Every palette row title the open palette is offering, in cursor order.
    fn row_titles(&mut self) -> Vec<String> {
        let count = self
            .query("sprag_palette", "row_count")
            .ok()
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                self.query("sprag_palette", &format!("row.{i}"))
                    .ok()?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect()
    }

    /// Everything the GUI has written to stderr so far. Missing / unreadable reads as empty, which
    /// the caller must treat as "no evidence", never as "no problem".
    fn gui_log(&self) -> String {
        std::fs::read_to_string(&self.gui_log).unwrap_or_default()
    }

    /// What pane tile `i` is SHOWING, one string per painted row.
    fn pane_rows(&mut self, i: usize) -> Vec<String> {
        self.tags()
            .remove(&format!("sprag_gui.pane.{i}"))
            .map(|pane| pane.rows)
            .unwrap_or_default()
    }

    /// How many frames the client has presented.
    fn frame_count(&mut self) -> u64 {
        self.call("scene/frame_timings", json!({}))
            .ok()
            .and_then(|timings| timings["frame_count"].as_u64())
            .unwrap_or_default()
    }

    /// Run the `sprag` CLI against THIS run's daemon, answering what it printed.
    ///
    /// A genuinely different ingress from the scene socket everything else here drives: it reaches
    /// the daemon, and the client learns of it on its own poll with no dispatch of ours in front.
    /// That is the whole point where a per-frame cost is concerned — see
    /// [`check_terminal_output_never_reaches_the_shaper`] — and it is also the plain user path, so a
    /// check driven this way exercises the chain a person actually uses.
    fn cli(&mut self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(self.target.join("sprag"))
            .env("SPRAG_HOST_RPC_SOCK", &self.host_sock)
            .args(args)
            .output()
            .map_err(|error| format!("sprag {args:?}: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(format!(
                "sprag {args:?} exited {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// A second connection, to the DAEMON's own socket rather than to the client's scene.
    ///
    /// The daemon serves a scene of its own — the session's panes, addressed by the ids it minted —
    /// and it is the only place a pane's INPUT can be written: the client's socket answers
    /// `NoExternalAtPath` for the same path, because the input external belongs to the host.
    fn daemon(&self) -> Result<HostConn, String> {
        HostConn::connect(&self.host_sock, PATIENCE).map_err(|error| error.to_string())
    }

    /// Watch every frame the client paints, from the one standing at `from` until `arrived`.
    ///
    /// Sampled rather than summed, because the per-frame count is only ever the LAST frame's. The
    /// loop reads the cheap counter continuously and the expensive arrival condition only when a new
    /// frame appears — which keeps the sampling dense enough for [`FrameWatch::contiguous`] to be a
    /// real guarantee rather than an optimistic one, and keeps a snapshot from landing between every
    /// pair of samples.
    ///
    /// Nothing in here can disturb what it measures: a read is served from the stored scene, so the
    /// polling neither re-derives nor stores a mirror nor damages anything into a frame.
    fn watch_frames(
        &mut self,
        from: u64,
        mut arrived: impl FnMut(&mut Self) -> bool,
    ) -> FrameWatch {
        let mut watch = FrameWatch {
            frames: Vec::new(),
            arrived: false,
            contiguous: true,
        };
        let mut last = from;
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Ok(timings) = self.call("scene/frame_timings", json!({})) {
                let count = timings["frame_count"].as_u64().unwrap_or(last);
                if count != last {
                    watch.contiguous &= count == last + 1;
                    last = count;
                    watch
                        .frames
                        .push((count, timings["last"]["shape_misses"].as_i64()));
                    if arrived(self) {
                        watch.arrived = true;
                        return watch;
                    }
                }
            }
            if Instant::now() >= deadline {
                return watch;
            }
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

/// Spawn `binary` with the smoke's isolated environment, its stderr captured into `log`.
///
/// Stdout is discarded and stderr goes to a FILE rather than to the terminal: a child's tracing
/// interleaved with the check lines would bury them, but discarding it outright would put the one
/// class of claim that never reaches the RPC surface out of reach — a diagnostic the app emits
/// ABOUT itself. `pinion::shell`'s unsettled-frame warning is exactly that, and
/// [`check_every_painted_frame_settled`] is the reader.
///
/// `SPRAG_LOG` is left unset on purpose: the default filter is already `warn`, which is the level
/// that carries a diagnostic, and naming a directive here would silently decide what the next
/// reader of this log is allowed to see.
fn spawn(binary: &Path, host: &Path, gui: &Path, state: &Path, log: &Path) -> io::Result<Child> {
    let log = std::fs::File::create(log)?;
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
        .stderr(log)
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
                rows: subtree_rows(node),
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

/// Every grid row painted anywhere in `node`'s subtree (see [`Painted::rows`]).
///
/// The subtree for the same reason the text walk uses it: the pane's tag is on a container and the
/// rows hang off its grid child, so a node matched by pane tag carries none of them itself.
fn subtree_rows(node: &Value) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(rows) = node["grid_rows"].as_array() {
        found.extend(
            rows.iter()
                .filter_map(|row| row["text"].as_str().map(str::to_owned)),
        );
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            found.extend(subtree_rows(child));
        }
    }
    found
}

/// The frames a client painted across one change, and what each spent on the shaper.
///
/// A run of samples rather than a total, because the per-frame count is the LAST frame's and pinion
/// keeps no cumulative paint-side sum. Everything a caller needs to know how far to trust it is
/// here: which frames were seen, whether any were missed, and whether the change arrived at all.
struct FrameWatch {
    /// `(frame_count, last.shape_misses)` for every frame seen, in order.
    ///
    /// The miss count is an OPTION and stays one: an absent field must never read as a zero, or the
    /// day upstream renames it every claim in sight passes for free.
    frames: Vec<(u64, Option<i64>)>,
    /// Whether the change under test reached the paint before the patience ran out.
    arrived: bool,
    /// Whether every frame in the span was actually seen.
    ///
    /// A `frame_count` that advances by more than one between samples means a frame was painted and
    /// never read — so a claim quantified over "every frame" is covering one it does not have. The
    /// sampler cannot prevent that; it can refuse to hide it.
    contiguous: bool,
}

impl FrameWatch {
    /// An empty span, to fold several drives' worth of watching into.
    ///
    /// `arrived` starts TRUE because folding is a conjunction — a span of many drives arrived only
    /// if each of them did, and an accumulator that started false could never say so.
    fn span() -> Self {
        Self {
            frames: Vec::new(),
            arrived: true,
            contiguous: true,
        }
    }

    /// Fold one drive's watch into this span.
    fn absorb(&mut self, other: Self) {
        self.frames.extend(other.frames);
        self.arrived &= other.arrived;
        self.contiguous &= other.contiguous;
    }

    /// Whether any frame in the span handed the shaper a run.
    fn shaped(&self) -> bool {
        self.frames
            .iter()
            .any(|(_, misses)| misses.is_some_and(|count| count > 0))
    }

    /// What each frame spent, for the report line.
    fn misses(&self) -> Vec<Option<i64>> {
        self.frames.iter().map(|&(_, misses)| misses).collect()
    }
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
