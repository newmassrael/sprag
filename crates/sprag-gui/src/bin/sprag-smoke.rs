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
            check_the_sole_docked_pane_locks_its_tear_off(&mut smoke, &mut report);
            check_focus_survives_a_window_change(&mut smoke, &mut report);
            check_a_pane_can_be_created_and_closed(&mut smoke, &mut report);
            // AFTER a check that answers a confirmation, and THAT ordering is load-bearing: the
            // state a confirmed row leaves behind is the whole claim. It replaces the opposite
            // constraint this list used to carry, when every focus-needing check had to run before
            // the first confirmation because one leaked the palette's modal scope for good.
            check_a_confirmed_row_leaves_the_focus_stack_clean(&mut smoke, &mut report);
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
    // a modal in it. A palette row closes its dialog in the same dispatch as the command, and
    // pinion's modal pop RESTORES the tag that was focused when the palette opened; the drain order
    // is focus-then-modal, so that restore lands after the op's own request whatever the op asked
    // for. Asserted rather than reasoned about, because "the shell will override it" is exactly the
    // kind of claim that reads as certain and turns out to depend on which arm of `focus_set` ran.
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
