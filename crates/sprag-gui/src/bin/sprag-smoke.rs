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
            // H3 slice 5's own gate, and it belongs beside the check above for the same reason: it
            // drives the DAEMON's pane and waits for the CLIENT to paint the consequence, so it needs
            // the one-pane correspondence that check has just established — and it leaves the pane
            // set exactly as it found it.
            check_an_agents_state_reaches_the_painted_pane_title(&mut smoke, &mut report);
            // Beside the check above and for the same reason: it drives the DAEMON's pane and waits
            // for the CLIENT to paint the consequence, so it needs the one-pane correspondence that
            // check establishes. It leaves the pane set as it found it (a `cd` moves no pane).
            check_a_sessions_sampled_activity_reaches_its_painted_row(&mut smoke, &mut report);
            // The keymap gate, HERE because it splits: after the check above, which needs exactly one
            // pane, and before the one below, which leaves several standing.
            check_the_gui_follows_the_users_keymap(&mut smoke, &mut report);
            // Straight after the keymap gate, which leaves SEVERAL panes standing and has just put
            // the shipped default prefix back — so `C-b z` here is the table sprag ships. It
            // restores the pane set it found, for the check below that needs exactly one.
            check_a_pane_fills_the_window_from_a_row_and_a_key(&mut smoke, &mut report);
            // After the keymap gate, which leaves the shipped table in force — this check drives
            // the DEFAULT prefix, so it must not run while a user config has moved it.
            check_the_window_keys_reach_the_daemon(&mut smoke, &mut report);
            // Straight after the window keys, which leave the session on its FIRST window with an
            // extra one behind it — exactly the arrangement a reorder needs, so this check reads
            // that inheritance rather than building its own.
            check_the_order_keys_move_a_window_on_the_daemon(&mut smoke, &mut report);
            // Straight after the order keys, and for their reason: the DEFAULT prefix is in force.
            // It creates a second SESSION and leaves it standing, and leaves this client back on
            // the session it started from — so a later check reads the same windows it would have.
            check_the_session_keys_move_this_client(&mut smoke, &mut report);
            // Straight after the window keys, and for the same reason: it drives the DEFAULT
            // prefix. It RENAMES the current window and leaves it renamed, which every later check
            // survives because they read the window they are on rather than a fixed name — an
            // inheritance, stated here rather than left for the next author to discover.
            // Straight after the session keys, and it needs what they leave: this client back on
            // the session it started from, with the DEFAULT prefix in force. It makes a session of
            // its own to pick, leaves it standing, and puts this client back where it found it.
            check_the_chooser_opens_and_a_picked_row_moves_this_client(&mut smoke, &mut report);
            // Straight after the chooser, which leaves this client back where it started with the
            // DEFAULT prefix in force. It writes a config of its own and puts the shipped table
            // back before it returns, exactly as the keymap gate above does.
            check_a_key_that_finds_nothing_says_so_on_the_screen(&mut smoke, &mut report);
            // Straight after the message checks above, which have proved the strip works and left
            // the client on its own session with the shipped table in force. It BLURS the window
            // and puts the focus back before it returns, so every later check reads a focused one.
            check_a_message_follows_the_person_out_of_the_window(&mut smoke, &mut report);
            check_the_rename_key_asks_and_the_answer_reaches_the_daemon(&mut smoke, &mut report);
            // Straight after the renames, and for their reason: the DEFAULT prefix is in force. It
            // splits a pane and leaves BOTH standing with an uneven share — every later check
            // reads the panes it finds rather than a fixed count or a fixed width, which is the
            // same inheritance the rename check above states.
            check_the_resize_key_moves_a_boundary_on_the_daemon(&mut smoke, &mut report);
            // Straight after them, and for their reason: it drives the DEFAULT prefix. This one
            // leaves NOTHING behind — it opens a read-only panel and closes it — so unlike the two
            // above it hands the next check the arrangement it was given.
            check_the_key_table_opens_and_shows_the_table_in_force(&mut smoke, &mut report);
            // AFTER the key table, which leaves what it was given, and BEFORE the checks that want
            // exactly one pane — because this one REMOVES a pane, which is the state it hands on.
            // The resize check above left two standing, so there is a sibling for the guarded kill
            // to leave behind, which is what makes its no-consequence control non-vacuous.
            check_the_guarded_kill_key_asks_and_a_yes_reaches_the_daemon(&mut smoke, &mut report);
            // HERE for the same reason as the keymap gate above: it needs exactly ONE pane, so the
            // pane the daemon reports and the grid this window paints are unambiguously the same
            // one. It attaches a second CLIENT and takes it away again, leaving the pane set as it
            // found it.
            check_a_window_and_a_terminal_agree_on_one_pane_size(&mut smoke, &mut report);
            check_a_pinned_window_overrides_what_this_window_measured(&mut smoke, &mut report);
            check_the_resize_key_pins_this_windows_own_area(&mut smoke, &mut report);
            // AFTER every check that needs the client this run booted, because it REPLACES that
            // client twice. Before the session kill, which is happy to kill whichever session the
            // client it finds is attached to.
            check_the_gui_follows_the_users_font(&mut smoke, &mut report);
            // AFTER it, and that ordering is load-bearing twice over: the check above needs
            // exactly ONE pane to make the daemon-to-client pane correspondence unambiguous, and
            // this one SPLITS until the pane set can attribute its own cost. It also leaves those
            // panes standing, so nothing that counts panes may follow it.
            check_the_host_projects_panes_only_for_a_grid_reader(&mut smoke, &mut report);
            // AFTER the check above, and it needs exactly what that one leaves: SEVERAL panes in
            // one window. A break from a window's only pane moves it into a window of its own,
            // which is indistinguishable from nothing happening — so this check cannot build its
            // own fixture more cheaply than by inheriting one. It leaves a window behind, and
            // nothing after it counts windows.
            check_the_break_key_gives_a_pane_a_window_of_its_own(&mut smoke, &mut report);
            // Straight before the check below, which needs the DEFAULT destroy policy: this one
            // writes `detach-on-destroy = "next"`, proves the windowed client MOVES rather than
            // leaving, and puts an empty `[options]` back before it returns. It leaves this client
            // on a DIFFERENT session than it found it on, which the check below discovers rather
            // than assumes.
            check_a_destroyed_session_moves_the_windowed_client(&mut smoke, &mut report);
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
/// The `open` verb is what makes this reachable at all: the palette's only other entry is a chord.
///
/// The reason recorded here used to be that synthetic key input does not drain headless. **That is no
/// longer true and was measured false this round** — [`check_the_gui_follows_the_users_keymap`] drives
/// `scene/key` and reaches `apply_key`, which is how the keymap gate works at all. What remains true
/// is narrower and still enough: a chord is a KEY, so driving the palette that way would assert the
/// chord table rather than the palette, and the verb is the surface an agent has either way.
///
/// The neighbouring claim about POINTER input ([`check_an_agents_read_costs_no_scene_rederive`]) rests
/// on the same inbox and is therefore doubtful too — but it was not measured here, and it is left
/// standing rather than quietly reworded.
fn check_the_palette_opens_over_rpc(smoke: &mut Smoke, report: &mut Report) {
    // `is_ok_and`, not `map_or(true, ..)`: this is the assertion an unreadable tree used to PASS
    // for the wrong reason, because "no palette node" and "no answer" were the same value.
    report.check(
        "the palette starts unpainted",
        smoke
            .tags()
            .is_ok_and(|painted| !painted.contains_key("sprag_palette_panel")),
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

/// **A HUMAN can fill the window with one pane, and get the arrangement back** — with a real key,
/// against real pixels.
///
/// The gap this closes is a whole feature's worth: the daemon has had a zoom since R285 and every
/// client HONOURED one, while nothing a person could press or click SET one. The only caller was
/// `sprag zoom-pane` — a shell command, for a feature whose entire subject is what the window looks
/// like.
///
/// Placed here because it needs SEVERAL panes standing and the keymap gate above leaves them, and
/// because that gate has just re-written the user's config back to the default prefix — so `C-b z`
/// is the shipped default table, not a fixture this check installed for itself.
///
/// # What makes it discriminating
///
/// The target is the LAST painted tile, never the first: a zoom that filled the window with
/// whichever pane came first would pass on a target of `0` and fail here. The assertion is then that
/// exactly ONE tile paints and it is THAT one — the pane the user was on — and that its rect GREW,
/// which is the difference between "the client redrew" and "the client fullscreened the pane".
///
/// The un-zoom is asserted with the same key, because that is the whole claim of a toggle: one
/// binding, both directions. A check that only zoomed would leave the window filled for every check
/// below it, which is also why this one restores what it found.
fn check_a_pane_fills_the_window_from_a_row_and_a_key(smoke: &mut Smoke, report: &mut Report) {
    // THE TABLE IN FORCE HERE IS NOT THE SHIPPED ONE, and this line is the whole reason this check
    // has a config write at all. The keymap gate above deliberately ENDS on a config the client
    // cannot use, so what is in force is the last file that parsed — which moved the prefix to
    // `C-a`. A check that pressed `C-b` on that inheritance would fail while the product worked,
    // which is exactly what the first run of this one did. Writing a usable file naming the default
    // prefix makes the binding under test the SHIPPED `z` and nothing about it inherited. Safe to
    // leave behind: the next check that reads this file writes its own.
    if smoke
        .write_user_config("[options]\nprefix = \"C-b\"\n")
        .is_err()
    {
        report.check("a usable config can be written to zoom under", false);
        return;
    }
    let Ok(before) = smoke.docked_panes() else {
        report.check("the painted tree answers a pane list to zoom within", false);
        return;
    };
    let Some(&target) = before.last() else {
        report.check("the window paints a pane to zoom", false);
        return;
    };
    if before.len() < 2 {
        report.check(
            &format!(
                "a zoom needs a window of more than one pane (found {})",
                before.len()
            ),
            false,
        );
        return;
    }
    // A keystroke goes to the FOCUSED pane, and the target is deliberately not the first one.
    if !smoke.focus_pane(target) {
        report.check("a pane can be focused to zoom", false);
        return;
    }
    let was = smoke
        .tags()
        .ok()
        .and_then(|tags| tags.get(&format!("sprag_gui.pane.{target}"))?.rect);

    // THE PALETTE first, because the two affordances are each other's control: a failure in only
    // one of them names the surface, while a failure in both names the client's host call.
    if !smoke.run_palette_row("Zoom pane to fill the window", report) {
        return;
    }
    let filled = smoke.wait_for(|s| {
        let panes = s.docked_panes().ok()?;
        (panes == vec![target]).then_some(panes)
    });
    report.check(
        &format!("the palette row left ONE pane painted, and it is the focused one ({filled:?})"),
        filled.is_ok(),
    );
    if filled.is_err() {
        // The DISCRIMINATOR the bare timeout lacks: ask the DAEMON, over the CLI verb written for
        // exactly this question, whether a pane fills the window. "The key never arrived" and "the
        // key arrived and this client did not redraw" are opposite defects in different crates, and
        // a timeout alone cannot tell them apart — which is a whole diagnosis, next time.
        let daemon = smoke
            .attached_session()
            .map(|session| smoke.cli(&["layout", "-t", &session]));
        report.check(
            &format!("...and the DAEMON's own reading says which half failed: {daemon:?}"),
            false,
        );
        return;
    }
    let now = smoke
        .tags()
        .ok()
        .and_then(|tags| tags.get(&format!("sprag_gui.pane.{target}"))?.rect);
    report.check(
        &format!("...and that pane GREW to fill the window ({was:?} -> {now:?})"),
        match (was, now) {
            (Some((was_w, was_h)), Some((now_w, now_h))) => {
                now_w * now_h > was_w * was_h && now_w >= was_w && now_h >= was_h
            }
            _ => false,
        },
    );

    // ...and the BOUND KEY back, which is both halves of the claim at once: the toggle gives the
    // arrangement back, and the key reaches the same command the row does.
    report.check(
        "the default prefix is accepted",
        smoke.press(target, "b", true).is_ok(),
    );
    report.check(
        "and `z` after it is too",
        smoke.press(target, "z", false).is_ok(),
    );
    let restored = smoke.wait_for(|s| {
        let panes = s.docked_panes().ok()?;
        (panes == before).then_some(panes)
    });
    report.check(
        &format!("`prefix z` gave the arrangement back ({restored:?})"),
        restored.is_ok(),
    );
    if restored.is_err() {
        // Both forks at once, because a key that does nothing has two of them and they live in
        // different crates: WHERE the client thinks the focus is (a keystroke reaches the keymap
        // only from a pane's own focus), and whether the DAEMON was asked at all.
        let focused = smoke.focused();
        let log = smoke.gui_log();
        let chords = log.lines().filter(|l| l.contains("chord")).count();
        let zooms = log.lines().filter(|l| l.contains("zoom-pane")).count();
        let daemon = smoke
            .attached_session()
            .map(|session| smoke.cli(&["layout", "-t", &session]));
        report.check(
            &format!(
                "...focus was {focused:?}, the client traced {chords} chord(s) of which {zooms} \
                 zoom, and the daemon reads {daemon:?}"
            ),
            false,
        );
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
    // Every count below is a DELTA against this one, so an unreadable tree here is not a pane
    // count of zero — it is a baseline this whole check would otherwise measure against.
    let Ok(before) = smoke.pane_count() else {
        report.check(
            "the client's painted tree answers a pane count to start from",
            false,
        );
        return;
    };
    report.check(
        &format!("the window starts with {before} pane(s)"),
        before > 0,
    );
    // `Kill pane` acts on the focused pane, so it is only OFFERED with one focused — and the pane
    // set has moved under the client since anything last held focus ([`Smoke::focus_pane`]).
    if let Ok(panes) = smoke.docked_panes()
        && let Some(&first) = panes.first()
    {
        report.check(
            "a pane can be focused to be killed",
            smoke.focus_pane(first),
        );
    }

    if !smoke.run_palette_row("Split into a new pane", report) {
        return;
    }
    // In a wait, an unreadable tree is honestly NOT YET: the next poll re-asks, and a tree that
    // stays unreadable becomes a timeout rather than a wrong count.
    let grown = smoke.wait_for(|s| {
        let count = s.pane_count().ok()?;
        (count > before).then_some(count)
    });
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
        smoke.pane_count().is_ok_and(|count| count > before),
    );

    report.check(
        "the prompt is answerable over RPC",
        smoke.invoke("sprag_confirm", "accept", Value::Null).is_ok(),
    );
    let shrunk = smoke.wait_for(|s| {
        let count = s.pane_count().ok()?;
        (count == before).then_some(count)
    });
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

    let Some(pane) = smoke
        .docked_panes()
        .ok()
        .and_then(|panes| panes.first().copied())
    else {
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
    let Ok(before) = smoke.tabs() else {
        report.check(
            "the client's painted tree answers a tab strip to start from",
            false,
        );
        return;
    };
    report.check(
        &format!("the strip has a tab to start from ({before:?})"),
        !before.is_empty(),
    );

    if !smoke.run_palette_row("New window", report) {
        return;
    }
    let Ok(grown) = smoke.wait_for(|s| {
        let tabs = s.tabs().ok()?;
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
        smoke.tabs().is_ok_and(|tabs| tabs.len() == grown.len()),
    );
    report.check(
        "the prompt is answerable over RPC",
        smoke.invoke("sprag_confirm", "accept", Value::Null).is_ok(),
    );

    let shrunk = smoke.wait_for(|s| {
        let tabs = s.tabs().ok()?;
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
    report.check(
        "a pane can be focused to act on",
        smoke.focus_first_pane().is_some(),
    );
    if !smoke.run_palette_row("Split into a new pane", report) {
        return;
    }
    let Ok(docked) = smoke.wait_for(|s| {
        let panes = s.docked_panes().ok()?;
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
        let panes = s.docked_panes().ok()?;
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
            (s.docked_panes().is_ok_and(|panes| panes.len() == 2)
                && s.panel_is_movable(remaining) == Some(true))
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
    let Ok(docked) = smoke.docked_panes() else {
        report.check("the client's painted tree answers a docked pane set", false);
        return;
    };
    let Some(&parked) = docked.last() else {
        report.check("a docked pane to park the focus ring on", false);
        return;
    };
    report.check(
        &format!("the ring parks on the highest docked pane (pane {parked} of {docked:?})"),
        smoke.focus_pane(parked) && parked > 0,
    );
    let Ok(home) = smoke.tabs() else {
        report.check("the client's painted tree answers a tab strip", false);
        return;
    };

    report.check(
        "the strip's + button activates",
        smoke
            .invoke(NEW_WINDOW_TAG, "send", json!("KeyboardActivate"))
            .is_ok(),
    );
    let Ok(grown) = smoke.wait_for(|s| {
        let tabs = s.tabs().ok()?;
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
        s.docked_panes().ok()?.contains(&index).then_some(focused)
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
        let panes = s.docked_panes().ok()?;
        (panes.contains(&index) && panes.len() == docked.len()).then_some(focused)
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
        s.docked_panes().ok()?.contains(&index).then_some(focused)
    });
    report.check(
        &format!("a live pane holds the keyboard after a PALETTE window change ({after:?})"),
        after.is_ok(),
    );
}

/// **R326's windowed half: a session destroyed under this client MOVES it, and it says so.**
///
/// # Why this front needs its own gate at all
///
/// It was the front that WORKED. The measured defect was `sprag-tui`'s — four of the five
/// `detach-on-destroy` values did nothing there — and `sprag-gui` had called the resolve from its
/// frame loop since R176. Gating only the front that was broken would be gating the fix rather than
/// the property, and this project has already paid for that once: R318's live smoke found a defect
/// the terminal front's pty gate structurally could not reach.
///
/// It is also the front where the SENTENCE is new. The resolve moved a person silently here for as
/// long as it has existed — the session rail simply changed under them — so the strip assertion
/// below is not a re-test of the terminal front's, it is the first test of this one's.
///
/// # The landing is DISCOVERED, never predicted
///
/// `next` walks the daemon's own creation order, and by this point in the run several checks have
/// left sessions behind — so naming the survivor here would be asserting the order of everything
/// that ran before. What is asserted instead is stronger and order-free: **the sentence names the
/// session the client is actually on**, which a client that moved somewhere else and said the wrong
/// name would fail.
fn check_a_destroyed_session_moves_the_windowed_client(smoke: &mut Smoke, report: &mut Report) {
    if smoke
        .write_user_config("[options]\ndetach-on-destroy = \"next\"\n")
        .is_err()
    {
        report.check("a destroy policy can be written", false);
        return;
    }
    // From here the config is NOT the shipped one, so every exit runs through the restore below.
    // The check that follows this one needs the DEFAULT policy — it asserts that a client LEAVES —
    // and an early return that left `next` in force would make it switch instead and report a
    // failure that is this function's, in that function's name.
    run_the_destroyed_session_checks(smoke, report);
    if smoke.write_user_config("[options]\n").is_err() {
        report.check("the default destroy policy can be restored", false);
    }
}

/// [`check_a_destroyed_session_moves_the_windowed_client`]'s body, split out so its early returns
/// cannot skip that function's restore.
fn run_the_destroyed_session_checks(smoke: &mut Smoke, report: &mut Report) {
    let Some(mine) = smoke.attached_session() else {
        report.check("the client says which session it is attached to", false);
        return;
    };
    // A guaranteed survivor, so this check does not depend on what earlier ones left standing.
    let spare = smoke.cli(&["new", "smoke-spare"]);
    report.check(
        &format!("a spare session exists to land in ({spare:?})"),
        spare.is_ok(),
    );

    // OUT OF BAND: the `sprag` CLI, not this client's palette. The distinction is the whole subject
    // — a gesture gets its own answer, and this is the path where nobody at this keyboard acted.
    let killed = smoke.cli(&["kill-session", &mine]);
    report.check(
        &format!("the CLI destroys the session this client is attached to ({killed:?})"),
        killed.is_ok(),
    );

    // The STRIP first, because it is the transient: the sentence expires on `display-time` while
    // the session rail keeps its new name forever, so a check that read the rail first could find
    // the message already gone and call it an absence.
    let said = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        let strip = tags.get("sprag_message_strip")?;
        let text = strip.text.join("\u{1f}");
        text.contains("was destroyed").then_some(text)
    });
    report.check(
        &format!("the client SAYS its session was destroyed ({said:?})"),
        said.as_deref().is_ok_and(|text| text.contains(&mine)),
    );

    let moved = smoke.wait_for(|s| s.attached_session().filter(|now| *now != mine));
    report.check(
        &format!("...and it MOVED rather than sitting on a session that is gone ({moved:?})"),
        moved.is_ok(),
    );
    // THE SENTENCE AGREES WITH THE RAIL. A client that moved and named somewhere else would pass
    // both checks above and still be lying to the person reading the strip.
    report.check(
        "...and the sentence names the session it actually landed on",
        match (&said, &moved) {
            (Ok(text), Ok(now)) => text.contains(now.as_str()),
            _ => false,
        },
    );
    report.check(
        "...and the client is still running, which is what a SWITCH policy means",
        !smoke.gui_exited(),
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
        smoke
            .tags()
            .is_ok_and(|painted| painted.contains_key("sprag_find")),
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
/// **H3 slice 5's gate**: a pane whose screen says an agent is waiting for an answer turns into TEXT
/// this client PAINTS — the state beside the pane's own title, where a person looking at the window
/// finds it.
///
/// # Why this has to be the live check and not a unit test
///
/// The verdict crosses five hands before a pixel: the daemon's detector produces it, the pane list
/// carries it, `sprag-client` parses it onto its poll cache, the title SSOT frames it, and every title
/// surface renders that string. Each hand has its own unit test and all of them pass while a pane
/// title says nothing — which is exactly the failure R253 recorded at the neighbouring seam
/// (unit-green, inert in the binary). Only a painted frame closes it.
///
/// # What is driven, and why it is not a real agent
///
/// The pane is made agent-SHAPED by typing a `printf` at its own shell: `claude`'s resting glyph in the
/// title, a bottom-anchored numbered choice list (what the `dialog-choice-list` rule reads), and the
/// footer its fingerprint matches. Every H3 measurement has been taken this way and the reason is
/// recorded in the design: a gate that needs a credential is a gate that gets skipped. The footer is
/// included deliberately — a shell that rewrites the title on its next prompt would take the
/// title-shaped fingerprint away, and the footer keeps the pane CLAIMED either way.
///
/// `Blocked` rather than `Idle`, because a verdict resting on evidence PRESENT on the screen publishes
/// on sight (D5's asymmetry): no settle window, so this check needs no sleep and cannot flake on one.
///
/// The phrase asserted is the one a person reads — "needs an answer", not the wire's `blocked` — so
/// this also gates the wording rule the two frontends share (`sprag_client::agent_phrase`).
fn check_an_agents_state_reaches_the_painted_pane_title(smoke: &mut Smoke, report: &mut Report) {
    /// The phrase a blocked agent's title must carry, rendered for a person rather than as the wire
    /// token — see [`sprag_client::agent_phrase`].
    const PHRASE: &str = "claude needs an answer";

    let Some(session) = smoke.attached_session() else {
        report.check("the client says which session to drive", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the daemon takes a second connection to drive it by", false);
        return;
    };
    // One pane on each side, waited on rather than sampled — the same correspondence argument (and the
    // same flake) as the check above: the daemon addresses a pane by id, the client paints it by index,
    // and nothing on the wire maps one to the other.
    let ids = daemon_panes(&mut daemon, &session);
    let one_each = smoke.wait_for(|s| {
        let painted = s.docked_panes().ok()?;
        matches!((ids.as_slice(), painted.as_slice()), ([_], [_])).then_some(painted)
    });
    report.check(
        &format!("one pane on each side to drive an agent screen into (daemon {ids:?})"),
        one_each.is_ok(),
    );
    let Ok(painted) = one_each else {
        return;
    };
    let ([id], [_index]) = (ids.as_slice(), painted.as_slice()) else {
        return;
    };
    let id = *id;

    // The title is not painted before the pane says anything — asserted, because a check whose
    // "after" state was already true before the drive proves nothing about the drive.
    let already = smoke
        .tags()
        .map(|tags| {
            tags.values()
                .any(|node| joined(&node.text).contains(PHRASE))
        })
        .unwrap_or(false);
    report.check(
        "no pane claims an agent state before one is painted",
        !already,
    );

    let script = "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\
                  \\342\\235\\257 1. Yes\\n  2. No\\n? for shortcuts\\n'\n";
    let drove: Result<Value, _> = daemon.call(
        "scene/invoke",
        json!({
            "path": format!("/pane_{id}/sprag_input/external/text"),
            "args": { "text": script },
            "session": session,
        }),
    );
    // Waited on the CONDITION — the painted text — and not on the drive's own answer: the text action
    // returns as soon as the bytes reach the PTY, which is three processes away from a frame.
    let titled = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        tags.iter()
            .find(|(_, node)| joined(&node.text).contains(PHRASE))
            .map(|(tag, node)| format!("{tag}: {:?}", node.text))
    });
    report.check(
        &format!("a blocked agent's state is PAINTED beside its pane's title ({titled:?}, drive {drove:?})"),
        titled.is_ok(),
    );
    // ...and the AT hears it too, through the same string — the one surface a sighted check cannot
    // stand in for, and the reason the marker is words rather than a glyph.
    let announced = smoke
        .access()
        .values()
        .filter_map(|node| node["name"].as_str().map(str::to_owned))
        .find(|name| name.contains(PHRASE));
    report.check(
        &format!("and a screen reader is told the same thing ({announced:?})"),
        announced.is_some(),
    );
}

/// Every string a node painted, as one haystack — the subtree text of a tagged node arrives as a list
/// (a title is one entry, the rows beside it others), and a phrase is looked for across the lot.
fn joined(text: &[String]) -> String {
    text.join(" ")
}

/// R282's whole chain, in pixels: a fact SAMPLED from the operating system by the daemon reaches the
/// session rail's subtitle over its own wire address, through a client mirror, joined onto the right
/// row by name.
///
/// Until R282 the session list carried this fact, so a client got it for free — and paid a `/proc`
/// walk of the box for the privilege on every poll wake. Now it is a separate question the client
/// asks separately, which is four new places for it to be dropped: the daemon's sampler, the wire
/// address, the poll thread's mirror, and the join in the sidebar. Every one of them has a unit test;
/// none of those would notice if the rail simply stopped drawing the line.
///
/// The drive is a `cd` into a directory this run creates, holding a `.git/HEAD` that names a branch
/// no repository on this machine has. That makes the assertion unfalsifiable by accident: the string
/// cannot arrive from the box's own state, only from this pane's cwd being read and its `HEAD`
/// parsed. The CONTROL is the same assertion taken BEFORE the drive — a check whose "after" state
/// was already true proves nothing about the drive.
fn check_a_sessions_sampled_activity_reaches_its_painted_row(
    smoke: &mut Smoke,
    report: &mut Report,
) {
    let branch = format!("r282-{}", std::process::id());
    let dir = smoke.state.join(format!("worktree-{branch}"));
    let made = std::fs::create_dir_all(dir.join(".git")).and_then(|()| {
        std::fs::write(dir.join(".git/HEAD"), format!("ref: refs/heads/{branch}\n"))
    });
    report.check(
        &format!("a work tree to drive a pane into ({made:?})"),
        made.is_ok(),
    );
    if made.is_err() {
        return;
    }

    let Some(session) = smoke.attached_session() else {
        report.check("the client says which session to drive", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the daemon takes a second connection to drive it by", false);
        return;
    };
    let ids = daemon_panes(&mut daemon, &session);
    let Some(&id) = ids.first() else {
        report.check(&format!("a pane to drive (daemon {ids:?})"), false);
        return;
    };

    // The control: nothing paints this branch before the pane is in that work tree.
    let already = smoke
        .tags()
        .map(|tags| {
            tags.values()
                .any(|node| joined(&node.text).contains(&branch))
        })
        .unwrap_or(false);
    report.check(
        "no session row claims the branch before a pane is working on it",
        !already,
    );

    let cd = format!("cd {}\n", dir.display());
    let drove: Result<Value, _> = daemon.call(
        "scene/invoke",
        json!({
            "path": format!("/pane_{id}/sprag_input/external/text"),
            "args": { "text": cd },
            "session": session,
        }),
    );
    // Waited on the painted CONDITION, not on the drive's answer: the text action returns when the
    // bytes reach the PTY, and between there and a painted subtitle lie a shell's chdir, the
    // daemon's next sample (bounded by the display tolerance, not by this call), the client's next
    // poll wake, and a frame.
    let painted = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        tags.iter()
            .find(|(tag, node)| {
                tag.starts_with("sprag_gui.stab.") && joined(&node.text).contains(&branch)
            })
            .map(|(tag, node)| format!("{tag}: {:?}", node.text))
    });
    // The DAEMON's own answer, asked directly — the discriminator that splits this chain in half. If
    // the sample carries the branch and the rail does not, the client's mirror or its join is at
    // fault; if the sample does not carry it either, nothing downstream can be blamed.
    let sampled: Result<Value, _> = daemon.call(
        "scene/query",
        json!({ "path": "/sprag_mux/external/session_activity.0" }),
    );
    // What the rail actually paints, reported WITH the verdict rather than left for a rerun: a gate
    // that cannot say what it saw cannot be debugged, and this one has four places to break.
    let rows: Vec<String> = smoke
        .tags()
        .map(|tags| {
            tags.iter()
                .filter(|(tag, _)| tag.starts_with("sprag_gui.stab."))
                .map(|(tag, node)| format!("{tag}={:?}", node.text))
                .collect()
        })
        .unwrap_or_default();
    report.check(
        &format!(
            "the sampled branch is PAINTED on the session's own row ({painted:?}, drive {drove:?}, pane {id} of {ids:?} in {session}, rail {rows:?}, daemon {sampled:?})"
        ),
        painted.is_ok(),
    );
    // ...and the same string reaches the screen reader, through `sidebar_access_name` rather than
    // through the painted subtitle — two readers of one sample, and the rail is the one surface
    // where a sighted check cannot stand in for the announced one.
    let announced = smoke
        .access()
        .values()
        .filter_map(|node| node["name"].as_str().map(str::to_owned))
        .find(|name| name.contains(&branch));
    report.check(
        &format!("and a screen reader is told the same thing ({announced:?})"),
        announced.is_some(),
    );
}

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
    //
    // WAITED ON, not sampled once. The client learns of the pane set on its own poll, so the frame
    // this reads may be one taken before the previous check's teardown had landed — and a single
    // sample of a set that is still settling reported `painted []` about one run in ten, which is
    // the flake this replaced. `.ok()?` makes an unreadable tree a retry rather than a count of
    // zero; if it stays unreadable, the failure below prints WHY instead of a bare empty list.
    let ids = daemon_panes(&mut daemon, &session);
    let one_each = smoke.wait_for(|s| {
        let painted = s.docked_panes().ok()?;
        matches!((ids.as_slice(), painted.as_slice()), ([_], [_])).then_some(painted)
    });
    // On failure the tree is read ONE more time for the message, so the diagnostic carries what was
    // finally there — an empty set, or the snapshot error that used to be indistinguishable from it.
    let last = match &one_each {
        Ok(painted) => Ok(painted.clone()),
        Err(_) => smoke.docked_panes(),
    };
    report.check(
        &format!(
            "the daemon and the client agree on ONE pane to drive (daemon {ids:?}, painted {last:?})"
        ),
        one_each.is_ok(),
    );
    // Nothing below may run on a guess: with the correspondence unproven, driving whichever pane the
    // ids happen to favour would let the claim pass or fail on which pane got the text.
    let Ok(painted) = one_each else {
        return;
    };
    let ([id], [index]) = (ids.as_slice(), painted.as_slice()) else {
        return;
    };
    let (id, index) = (*id, *index);

    // ── The detector: novel CHROME text, over the same host path, before anything is claimed.
    let from = smoke.frame_count();
    let renamed = smoke.cli(&["rename-window", NOVEL_WINDOW, "-t", &session]);
    let watch = smoke.watch_frames(from, |s| {
        s.tabs()
            .is_ok_and(|tabs| tabs.iter().any(|name| name == NOVEL_WINDOW))
    });
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
        // `is_ok_and(|rows| rows.iter()..)`, NOT `pane_rows(..).iter()..`: `Result` has an `iter`
        // too, of zero-or-one `Vec<String>`, so the shorter chain compiles and quietly asks whether
        // the row LIST equals the line rather than whether any ROW contains it. It was written that
        // way for one build here and the grid check caught it on the first run.
        watch.absorb(smoke.watch_frames(from, |s| {
            s.pane_rows(index)
                .is_ok_and(|rows| rows.iter().any(|row| row.contains(&line)))
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

/// **THE GATE for a setting a window adopts at BIRTH**: `gui-font` reaches the shipped client, and
/// the glyph size it names is the one every pane's grid is measured at.
///
/// A boot-adopted option cannot be gated the way a keymap is. Writing the file under a running window
/// proves nothing here — the client is not supposed to notice, so the check would pass identically
/// against one that never read the option at all. So this LAUNCHES two clients and compares what the
/// daemon was told, which is the only observation that separates them.
///
/// The claim is quantitative and fails in the right direction: a doubled glyph in the same window is
/// FEWER columns, and a client ignoring the option reports the same number twice. Read through
/// `sprag panes`, so the value is the one the PTY was actually sized to rather than anything this file
/// computed.
///
/// Each launch gets its own session (a GUI creates one), so both readings are of a single boot pane at
/// the full window — the comparison the earlier checks cannot offer, since they leave panes tiled.
///
/// REVERT-PROOF: return `FONT_SIZE_PX` from `font_size_px` (i.e. never read the option) and the two
/// readings are equal.
fn check_the_gui_follows_the_users_font(smoke: &mut Smoke, report: &mut Report) {
    // The reference launch: no `gui-font`, so the registry default is in force.
    if smoke.write_user_config("").is_err() {
        report.check("the user config can be cleared", false);
        return;
    }
    let Some(default_cols) = boot_pane_cols(smoke, report, "at the default glyph size") else {
        return;
    };
    // ...and the same client again, with the option set to twice it.
    if smoke
        .write_user_config("[options]\ngui-font = 40\n")
        .is_err()
    {
        report.check("a gui-font can be written", false);
        return;
    }
    let Some(large_cols) = boot_pane_cols(smoke, report, "at a doubled glyph size") else {
        return;
    };
    report.check(
        &format!(
            "a bigger gui-font measures a NARROWER grid ({default_cols} -> {large_cols} columns)"
        ),
        // Not merely `<`: an off-by-one would satisfy that, and what a doubled glyph must produce is
        // roughly half the columns. Two thirds is the loose bound the exact cell metric may land in.
        large_cols * 3 < default_cols * 2,
    );
}

/// Relaunch the client and answer the columns its BOOT pane was sized to, or `None` after reporting.
fn boot_pane_cols(smoke: &mut Smoke, report: &mut Report, when: &str) -> Option<u16> {
    if let Err(error) = smoke.relaunch_gui() {
        report.check(&format!("the client relaunches {when} ({error})"), false);
        return None;
    }
    let session = smoke.attached_session();
    let listed = session
        .as_deref()
        .map(|session| smoke.cli(&["panes", "-t", session]));
    // `id: COLSxROWS  command` — the first line is the boot pane, the only one a fresh session has.
    let cols = listed
        .as_ref()
        .and_then(|listed| listed.as_ref().ok())
        .and_then(|listed| listed.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|dims| dims.split('x').next())
        .and_then(|cols| cols.parse::<u16>().ok())
        .filter(|cols| *cols > 0);
    report.check(
        &format!("the relaunched client's boot pane reports a grid {when} ({cols:?})"),
        cols.is_some(),
    );
    cols
}

/// **THE GATE for the GUI's half of the user keymap.** Every other test of it either seeds a table or
/// drives `route_key` in-process, and none can say whether the SHIPPED client ever looks at the user's
/// file — the seam where the whole feature can be absent with a green suite. This is the one place the
/// claim can be made: the real binary, a real daemon, and a `config.toml` this harness wrote into an
/// environment it owns.
///
/// The MIDDLE claims discriminate, and they fail in opposite directions:
/// * a bare `%` must divide nothing — so a client that treated every `%` as a command cannot pass;
/// * `C-b` must arm NOTHING, because the file moved the prefix to `C-a` — so a client holding
///   `Keymap::default()` cannot pass.
///
/// Slice 4's ROOT table is checked here too rather than in a gate of its own, because it is the same
/// claim about the same seam — this binary reads this file — and a second gate would pay the cost of
/// a second window to assert it. Its own discriminating claim is an UNBOUND function key dividing
/// nothing, immediately before the bound one divides.
///
/// Placed after the one-pane check above (it leaves an extra pane standing) and before the check
/// below, which splits without cleaning up.
fn check_the_gui_follows_the_users_keymap(smoke: &mut Smoke, report: &mut Report) {
    let written = smoke.write_user_config("[options]\nprefix = \"C-a\"\n");
    report.check(
        "a user config can be written for the client to read",
        written.is_ok(),
    );
    if written.is_err() {
        return;
    }
    let Ok(before) = smoke.pane_count() else {
        report.check("the painted tree answers a pane count to start from", false);
        return;
    };
    // A keystroke goes to the FOCUSED pane, so the focus has to be real (see [`Smoke::focus_pane`]).
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to type into", false);
        return;
    };

    report.check(
        "an unprefixed command key is accepted",
        smoke.press(pane, "%", false).is_ok(),
    );
    report.check(
        &format!("...and divides nothing ({before} pane(s))"),
        smoke.pane_count() == Ok(before),
    );

    // The DEFAULT prefix, which this user's file has moved: it arms nothing, so the key after it is
    // still the program's.
    let _ = smoke.press(pane, "b", true);
    let _ = smoke.press(pane, "%", false);
    report.check(
        "the default prefix arms nothing once the file has moved it",
        smoke.pane_count() == Ok(before),
    );

    // ...and the prefix the FILE names does.
    report.check(
        "the user's prefix is accepted",
        smoke.press(pane, "a", true).is_ok(),
    );
    report.check(
        "and the command key after it is too",
        smoke.press(pane, "%", false).is_ok(),
    );
    let grown = smoke.wait_for(|s| {
        let count = s.pane_count().ok()?;
        (count > before).then_some(count)
    });
    report.check(
        &format!("`prefix %` off the user's own config split the focused pane ({grown:?})"),
        grown.is_ok(),
    );

    // THE ROOT TABLE (slice 4), through the same shipped binary. `F5` is bound with `-n`, so it acts
    // with no prefix at all — and the discriminating claim is the one BEFORE it: an unbound function
    // key must divide nothing, so a client that acted on every key it did not recognise cannot pass.
    let grown = grown.unwrap_or(before);
    if smoke
        .write_user_config(
            "[[bind]]\nkey = \"F5\"\naction = \"split-window -h\"\ntable = \"root\"\n",
        )
        .is_err()
    {
        report.check("a root-table config can be written", false);
        return;
    }
    let _ = smoke.press(pane, "F6", false);
    report.check(
        &format!("an unbound key still divides nothing ({grown} pane(s))"),
        smoke.pane_count() == Ok(grown),
    );
    report.check(
        "a root-table key is accepted with no prefix",
        smoke.press(pane, "F5", false).is_ok(),
    );
    let rooted = smoke.wait_for(|s| {
        let count = s.pane_count().ok()?;
        (count > grown).then_some(count)
    });
    report.check(
        &format!("`-n F5` split the focused pane with NO prefix ({rooted:?})"),
        rooted.is_ok(),
    );

    // ...and the other half of reading a config: a file this client CANNOT use must say so where a
    // user will see it. A window has no screen to fail on, so the report goes to the palette beside
    // the one a broken project config gets — and this reads the PAINTED line, because a report only a
    // log holds is a keymap error nobody fixes.
    if smoke
        .write_user_config("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n")
        .is_err()
    {
        report.check("a broken user config can be written", false);
        return;
    }
    let reported = palette_text(smoke);
    report.check(
        &format!("a config the client cannot use is REPORTED in the palette ({reported:?})"),
        reported
            .as_deref()
            .is_some_and(|text| text.contains("config.toml") && text.contains("kill-server")),
    );

    // Put a usable config back and the report GOES — the palette re-reads the file when it opens, so a
    // user who fixes their typo is not still being told about it. Found by reading this line: with the
    // keystroke path as the only re-reader the report was permanent, because the palette's own field
    // holds the keyboard while it is open and no keystroke can reach a pane to clear it.
    let _ = smoke.write_user_config("[options]\nprefix = \"C-a\"\n");
    // A WAIT, because a single re-open can read the previous frame: closing and opening in quick
    // succession leaves `sprag_palette_panel` painted throughout, so `wait_for_tag` is satisfied by the
    // tree that still holds the old line. Each attempt opens AND closes, so the focus trap stays
    // balanced for the checks that follow (they need a pane focused to be offered pane rows).
    let fixed = smoke.wait_for(|s| {
        let text = palette_text(s)?;
        (!text.contains("config.toml")).then_some(text)
    });
    report.check(
        &format!("and it GOES when the file is fixed ({:?})", fixed.is_ok()),
        fixed.is_ok(),
    );

    // A broken OPTION is reported by the same surface as a broken binding, because the file is ONE
    // document with one verdict: the options and the keymap are validated together, so a client cannot
    // end up honouring the half of a config that happened to parse. Read painted, for the reason
    // above — and it is a distinct claim from the binding case, since a reader that skipped the
    // `[options]` table would leave this file looking perfectly usable.
    if smoke
        .write_user_config("[options]\ndetach-on-destroy = \"sideways\"\n")
        .is_err()
    {
        report.check("a broken option can be written", false);
        return;
    }
    let complaint = smoke.wait_for(|s| {
        let text = palette_text(s)?;
        text.contains("config.toml").then_some(text)
    });
    report.check(
        &format!("a value no option takes is REPORTED too ({complaint:?})"),
        complaint
            .as_deref()
            .is_ok_and(|text| text.contains("detach-on-destroy")),
    );
}

/// Open the palette, read every string its panel paints, and DISMISS it again.
///
/// The dismissal is a scrim click (`send("scrim:PointerUp")`) and its result is CHECKED, because there
/// is no `close` verb: an `invoke("close")` answers `Rejected`, and a caller that drops that gets a
/// palette which stays open, refuses every later `open` (the palette's `open_on_request` declines when it
/// is already up), and keeps painting the catalog frozen at the FIRST open. Three runs of this file
/// read that as a mechanism defect in the client. It is the failure [`Smoke::call`]'s own doc warns
/// about — a rejected call looks exactly like one that did nothing.
fn palette_text(smoke: &mut Smoke) -> Option<String> {
    smoke.invoke("sprag_palette", "open", Value::Null).ok()?;
    let painted = smoke.wait_for_tag("sprag_palette_panel").ok()?;
    let text = painted
        .get("sprag_palette_panel")
        .map(|node| node.text.join(" "));
    smoke
        .invoke("sprag_palette", "send", json!("scrim:PointerUp"))
        .ok()?;
    // The dismissal has to have LANDED before the next open, or that open is refused and the read
    // after it is this one's leftovers.
    smoke
        .wait_for(|s| {
            let painted = s.tags().ok()?;
            (!painted.contains_key("sprag_palette_panel")).then_some(())
        })
        .ok()?;
    text
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
        let _ = smoke.wait_for(|s| (s.pane_count().ok()? >= panes).then_some(()));
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

    // -- And the CLIENT's half. R217 measured an idle window at one whole pane set, because a poll
    // wake re-fetched every pane whether or not anything had happened in it. Now a wake fetches
    // only the panes whose projection token MOVED, so driving output into ONE pane must cost that
    // pane's area and nothing else -- the same divisibility argument as above, run the other way
    // round: a multiple of the DRIVEN pane, never of the set.
    let Some(before) = grid_work(&mut daemon, &session) else {
        report.check("the host reports its meter before the drive", false);
        return;
    };
    let needle = "l3-damage-driven";
    let driven = daemon.call(
        "scene/invoke",
        json!({
            "path": format!("/pane_{named}/sprag_input/external/text"),
            "args": { "text": format!("echo {needle}\n") },
            "session": session,
        }),
    );
    // CAUGHT UP, not "shows the needle" — and the difference is a whole class of false failure.
    // The pane is 152 cells; a shell that echoes a line and then prints its prompt can push that
    // line off its OWN screen before any client can paint it, and then no client is behind, there
    // is simply nothing left to show. Measured: this check hunted the bare string and failed about
    // one run in eight, always with the daemon's screen no longer holding it either — which a
    // parent-commit control reproduced exactly, because it was never about the code under test.
    //
    // So the condition is a fact that cannot expire: the client shows the line, OR the pane has
    // already scrolled it away. Both mean "not behind". The claim this check is really about — that
    // driving ONE pane costs that pane's area and nothing else — is priced below and is untouched.
    let painted = smoke
        .wait_for(|s| {
            let shown = (0..s.pane_count().ok()?).any(|pane| {
                s.pane_rows(pane)
                    .is_ok_and(|rows| rows.iter().any(|row| row.contains(needle)))
            });
            // Unknown (the daemon would not answer) counts as STILL SHOWABLE, so an unreadable
            // daemon can never be the reason this check passes.
            let showable = pane_holds(&mut daemon, &session, named, needle)
                .1
                .unwrap_or(true);
            (shown || !showable).then_some(())
        })
        .is_ok();
    // WHY it is behind, on the failing path only, asked of the DAEMON in the two ways that tell the
    // causes apart: a shell that never ran the line leaves the daemon's own text without it, while
    // a client that never fetched leaves the daemon's SCREEN holding it. Both reads are needed —
    // the scrollback-inclusive one alone cannot tell "lost" from "expired", which is the confusion
    // that hid this for two rounds.
    let reached_the_daemon = (!painted).then(|| pane_holds(&mut daemon, &session, named, needle));
    // And the last fork: drive a SECOND line and see whether THAT one lands. The client stays
    // responsive either way — it answers `pane_rows` throughout the timeout above — so "responsive"
    // says nothing about its poll thread. A second line that paints proves the thread is alive and
    // looping, which makes the first miss a FETCH DECISION; a second line that also never lands
    // proves the thread has stopped delivering, which is a different bug in a different place.
    let second_line_landed = (!painted).then(|| {
        let again = "l3-damage-again";
        let _ = daemon.call(
            "scene/invoke",
            json!({
                "path": format!("/pane_{named}/sprag_input/external/text"),
                "args": { "text": format!("echo {again}\n") },
                "session": session,
            }),
        );
        smoke
            .wait_for(|s| {
                (0..s.pane_count().ok()?)
                    .any(|pane| {
                        s.pane_rows(pane)
                            .is_ok_and(|rows| rows.iter().any(|row| row.contains(again)))
                    })
                    .then_some(())
            })
            .is_ok()
    });
    let Some(after) = grid_work(&mut daemon, &session) else {
        report.check("the host reports its meter after the drive", false);
        return;
    };
    let (projections, cells) = (after.0 - before.0, after.1 - before.1);
    // Non-vacuity first: a window in which nothing was painted prices nothing, and the
    // divisibility test below is trivially true of zero.
    //
    // `painted` is REPORTED and not merely conjoined, because the two ways this fails are opposite
    // and a bare `false` cannot tell them apart: the wait timing out means the needle never reached
    // the client's grid, while `painted` with no cells would mean it arrived without a fetch. A
    // 1-in-16 failure here was recorded as `0 projections, 0 cells` with nothing to say which — the
    // silence R247 removed from the cropped-pane gate, in a second place.
    report.check(
        &format!(
            "the driven line reached the client's painted grid \
             (invoke {driven:?}, painted {painted}, {cells} cells{})",
            match (reached_the_daemon, second_line_landed) {
                (None, _) => String::new(),
                (Some((Some(false), _)), _) => {
                    ", and the DAEMON's own pane does NOT — the shell never ran it".to_owned()
                }
                (Some((_, Some(false))), _) => ", and the DAEMON's own SCREEN does not hold it \
                     either — it scrolled out of this pane, so no client could have painted it"
                    .to_owned(),
                (Some((None, _) | (_, None)), _) => {
                    ", and the daemon would not say what its pane holds".to_owned()
                }
                (Some((Some(true), Some(true))), Some(true)) => {
                    ", and the DAEMON's screen holds it while a LATER line painted — the poll \
                     thread is alive, so this was a fetch decision"
                        .to_owned()
                }
                (Some((Some(true), Some(true))), _) => {
                    ", and the DAEMON's screen holds it and a LATER line did not paint either — \
                     the poll thread has stopped delivering"
                        .to_owned()
                }
            }
        ),
        painted && cells > 0,
    );
    report.check(
        &format!(
            "only the pane that CHANGED was re-fetched ({projections} projections, {cells} cells is {}x pane_{named}'s {one}, not a multiple of the {total}-cell set)",
            cells / one.max(1)
        ),
        cells.is_multiple_of(one) && !cells.is_multiple_of(total),
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

/// A window and a TERMINAL client viewing one session give its pane ONE size — the defect this
/// check exists for, measured before it was fixed.
///
/// Before: the window sized its panes from its own pixels and a terminal client sized the same
/// panes from the session's arbitrated window, so attaching one to a session a window was showing
/// changed the pane underneath it — measured at 38x17 becoming 49x30 while the window went on
/// painting 38x17 of it, thirteen rows and eleven columns off screen with nothing to say so.
///
/// The three claims, in the order a user meets them:
///
/// 1. This window REPORTS an area, so the arbitration can see it at all. Without this, no
///    `window-size` value the user could set would reach a GUI.
/// 2. Under `smallest`, attaching a 100x30 terminal leaves the pane at the size the window had
///    ALONE. That is the defect's exact negation.
/// 3. Under `largest`, the pane takes the terminal's area — the window then shows part of it, which
///    is what the user asked for by naming the policy, and is what tmux says of any client smaller
///    than the window.
///
/// Then the terminal DETACHES, and the pane returns to the window's own size. That one is the
/// subtlest of the four and the reason the derivation lives in the daemon: a departing client
/// changes the window, and the client that remains has no seam that could notice.
///
/// The sizes are asserted as RELATIONS (against what this window measured for itself, read live)
/// rather than as literals, because the cell metric depends on the font this run happens to
/// resolve — a check written to `38x17` would be asserting the fixture's font.
fn check_a_window_and_a_terminal_agree_on_one_pane_size(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon", false);
        return;
    };

    // What this window alone makes of the session: the panes it is showing, and the window the
    // daemon arbitrated from its report. Read rather than assumed, because the cell metric depends
    // on the font this run resolved and the pane set on the checks that ran before this one.
    let solo = settled_pane_dims(smoke, &mut daemon, &session);
    let alone = window_size(&mut daemon, &session);
    report.check("the window's session holds panes", !solo.is_empty());

    // CLAIM 0, and the one that gates the FOLD: alone, this window hides nothing. Its report is the
    // only input to the window, so a report larger than what it can draw comes straight back as
    // panes too big for their widgets — which is R241's stated reason a GUI could not simply be
    // handed the arbitrated window, and it would be silent without this line.
    //
    // ## The equality this used to assert was NOT ACHIEVABLE, and measuring it said so
    //
    // It read `painted == buffer` and failed at 123/1 the moment H7 made the client follow the
    // session's active pane, because that built `wide | (narrow | narrow)` where the run had built
    // `(narrow | narrow) | wide` — and there the daemon hands the wide pane 37 columns while this
    // client's widget spans 38. That is not a defect the equality could have caught, because it is
    // not a defect: `fit_window` folds each pane's MEASURED cells into one window and the daemon
    // re-divides that window by the same ratios, in cells rather than pixels. Two quantisations of
    // one boundary, so some pane keeps a cell of slack whenever they disagree — the shape decides
    // which pane, not the code.
    //
    // What `fit_window` DOES promise is the half that matters, and it promises it by construction:
    // it returns the largest window whose tiling gives no pane more cells than that pane's own
    // surface measured. So nothing is ever truncated, and the slack has a bound worth watching.
    // Both are asserted below; the retired equality was the aspiration, and these are the property.
    //
    // ## PINION-PR80 moved where the widget's span is read, and that is why it stayed a check
    //
    // The node used to DERIVE its `cols`/`rows` from its rect, so reading them was reading the
    // widget. It now declares the daemon's grid instead, which makes `cols == buffer_cols` true by
    // construction — so both checks below would have passed on `0 <= 0` forever while measuring
    // nothing. The span is read from `rect / cell metric` instead: the same quantity, computed
    // where it still lives, so the bound the equality could not be goes on being watched.
    let mut grids = Vec::new();
    let held = smoke
        .wait_for(|smoke| {
            let seen = smoke.grid_facts();
            let fits = !seen.is_empty()
                && seen
                    .iter()
                    .all(|g| g.buffer.0 <= g.widget.0 && g.buffer.1 <= g.widget.1);
            grids = seen;
            fits.then_some(())
        })
        .is_ok();
    // The grids as they actually stand — printed on EVERY run, not only a failing one, because the
    // slack is the number this pair of checks exists to keep visible and a gate that only speaks
    // when it breaks cannot show a bound drifting toward it.
    eprintln!("      grids (widget vs declared vs buffer): {grids:?}");
    report.check(
        "alone, no pane is painted smaller than the session gave it",
        held,
    );
    // The TIGHTER claim, and the one that would catch a fold going wrong rather than merely
    // rounding: the surplus a pane's widget carries over its grid is under one cell per axis. A
    // window folded from measurements it then over-divides would show up here as a pane whose
    // widget is cells wider than its content, long before anything looked wrong on screen.
    report.check(
        "alone, a pane's widget carries under one cell of surplus",
        held && grids
            .iter()
            .all(|g| g.widget.0 - g.buffer.0 <= 1 && g.widget.1 - g.buffer.1 <= 1),
    );
    // PINION-PR80's own acceptance, live: the node states WHO sized it, and the size it states is
    // the one the producer delivered. Before it, every tiled pane reported a permanent divergence
    // that pinion's interpretation rule defines as an in-flight resize or a producer bug — over
    // the very channel built for an AI client to read.
    report.check(
        "alone, a pane's grid names its daemon and matches what it delivered",
        held && grids
            .iter()
            .all(|g| g.source == "producer" && g.declared == g.buffer),
    );

    // CLAIM 1: this window is IN the arbitration, which it can only be by reporting — and as the
    // only client, its report IS the window. Before this round it reported nothing, and no
    // `window-size` value a user could set would have reached it.
    report.check(
        "the window reports its own cell area",
        client_sizes(&mut daemon, &session)
            .iter()
            .any(|size| size.is_some()),
    );
    report.check(
        "the session's window is what this client reported",
        alone.is_some()
            && client_sizes(&mut daemon, &session)
                .iter()
                .all(|size| *size == alone),
    );

    if let Err(error) = smoke.write_user_config("[options]\nwindow-size = \"smallest\"\n") {
        report.check("the smoke writes window-size", false);
        eprintln!("      {error}");
        return;
    }

    let terminal = match smoke.attach_terminal(&session, 100, 30) {
        Ok(terminal) => terminal,
        Err(error) => {
            report.check("a terminal client attaches to the window's session", false);
            eprintln!("      {error}");
            return;
        }
    };
    let attached = smoke
        .wait_for(|_| (client_sizes(&mut daemon, &session).len() == 2).then_some(()))
        .is_ok();
    if !attached {
        // The client is a PROCESS on a real pty, so the reason it did not register is on its own
        // screen and nowhere else — and swallowing it costs a whole round. Measured here: a
        // `sprag-tui` left over from before a `WIRE_PROTOCOL` bump is refused at the daemon's
        // door, and this check reported only "did not attach" while the sentence naming the
        // skew sat unread one function call away. Three checks downstream fail with it, so the
        // reader gets four silent failures and no cause.
        let screen = terminal.with_screen(|screen| {
            (0..screen.rows())
                .map(|row| screen.row_text(row).trim_end().to_owned())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" | ")
        });
        eprintln!("      the terminal client's own screen says: {screen}");
    }
    report.check(
        "a terminal client attaches to the window's session",
        attached,
    );

    // CLAIM 2: under `smallest` this window is the smaller client, so the window does not move and
    // NOTHING under it does either. That is the measured defect's exact negation — before, the
    // terminal's 100x30 became the panes' size and this window went on painting its own.
    report.check(
        "under `smallest` the window does not move",
        window_size(&mut daemon, &session) == alone,
    );
    report.check(
        "under `smallest` every pane keeps the size this window gave it",
        settled_pane_dims(smoke, &mut daemon, &session) == solo,
    );

    // CLAIM 3: the policy reaches the panes. The terminal is RESIZED rather than the config merely
    // rewritten, because a policy is read when a window is re-derived and nothing re-derives on a
    // file write — a check that only edited the file would pass against a daemon that never read it.
    let _ = smoke.write_user_config("[options]\nwindow-size = \"largest\"\n");
    let larger_pty = (120u16, 40u16);
    // What the terminal client REPORTS out of that pty: one row less, kept for its status line
    // (R316, `sprag_tui::Split`). The window is arbitrated over what clients report, never over
    // what their terminals are — so a check that expected the pty's own height would be asserting
    // that `sprag-tui` paints its status row over a pane's last line.
    let larger = (larger_pty.0, larger_pty.1 - 1);
    if let Err(error) = terminal.resize(larger_pty.0, larger_pty.1, (0, 0)) {
        eprintln!("      could not resize the terminal client: {error}");
    }
    report.check(
        "under `largest` the window takes the terminal's area",
        smoke
            .wait_for(|_| (window_size(&mut daemon, &session) == Some(larger)).then_some(()))
            .is_ok(),
    );
    let grown = settled_pane_dims(smoke, &mut daemon, &session);
    report.check(
        "and every pane grew with it",
        !grown.is_empty()
            && grown.iter().all(|(pane, (cols, rows))| {
                solo.get(pane)
                    .is_some_and(|(was, tall)| cols >= was && rows >= tall)
            })
            && grown != solo,
    );

    // And this window is now showing PART of a pane — the crop the policy asked for, which is only
    // honest because the other policy would have chosen this client's own area instead.
    report.check(
        "the window paints part of every pane, which are now larger than it",
        every_pane_is_cropped(smoke, "largest"),
    );

    // The detach: a client leaving moves the window as surely as one arriving, and the client that
    // REMAINS has no seam that could notice — which is why the derivation is the daemon's.
    drop(terminal);
    report.check(
        "the panes return to this window's own size when the terminal leaves",
        smoke
            .wait_for(|_| {
                let dims = pane_dims(&mut daemon, &session);
                (!dims.is_empty() && dims == solo).then_some(())
            })
            .is_ok(),
    );

    // Leave the file as this check found it, so a later one reads its own settings and not these.
    let _ = smoke.write_user_config("");
}

/// **`window-size manual` reaches a WINDOW, not only a terminal.**
///
/// The check above proves a GUI is IN the arbitration; this proves the one policy that takes the
/// arbitration AWAY from it still governs it. That is not the same claim, and it is the one a
/// reasoning-only argument would have skipped: a GUI reports what its panes measured, and `manual`
/// ignores that report, so "the pin wins" depends on the client NOT re-asserting its own measurement
/// — which is exactly the two-authority defect this front removed and would silently restore.
///
/// Four claims, and the last is the strongest:
///
/// * The panes take the PINNED size, which is deliberately LARGER than what this window measured, so
///   no fallback to the client's own area can be mistaken for a pass.
/// * This window then paints PART of every pane — the same crop path the `largest` check exercises,
///   reached from a stored decision instead of from another client's report.
/// * It HOLDS. A client that answered a too-large grid by re-reporting, or by sizing its own panes,
///   would pull the panes back within a frame or two.
/// * `-u` returns every pane to what this window measured. Only that direction can distinguish "the
///   pin governs a live report" from "the client stopped reporting at all".
fn check_a_pinned_window_overrides_what_this_window_measured(
    smoke: &mut Smoke,
    report: &mut Report,
) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon", false);
        return;
    };
    let solo = settled_pane_dims(smoke, &mut daemon, &session);
    let Some(measured) = window_size(&mut daemon, &session) else {
        report.check("this window reported an area to pin against", false);
        return;
    };

    if let Err(error) = smoke.write_user_config("[options]\nwindow-size = \"manual\"\n") {
        report.check("the smoke writes window-size manual", false);
        eprintln!("      {error}");
        return;
    }
    // Bigger than what this window can draw, in BOTH dimensions, so every later claim is a relation
    // against a number no client here reported.
    let pinned = (measured.0 + 20, measured.1 + 8);
    if let Err(error) = smoke.cli(&[
        "resize-window",
        "-t",
        &session,
        "-x",
        &pinned.0.to_string(),
        "-y",
        &pinned.1.to_string(),
    ]) {
        report.check("sprag resize-window pins the window", false);
        eprintln!("      {error}");
        let _ = smoke.write_user_config("");
        return;
    }
    report.check(
        "a pinned window becomes the session's window, over what this client reported",
        smoke
            .wait_for(|_| (window_size(&mut daemon, &session) == Some(pinned)).then_some(()))
            .is_ok(),
    );
    let grown = settled_pane_dims(smoke, &mut daemon, &session);
    report.check(
        "and every pane grew past what this window measured",
        !grown.is_empty()
            && grown != solo
            && grown.iter().all(|(pane, (cols, rows))| {
                solo.get(pane)
                    .is_some_and(|(was, tall)| cols >= was && rows >= tall)
            }),
    );
    report.check(
        "this window paints part of every pinned pane",
        every_pane_is_cropped(smoke, "manual"),
    );
    // ...and it STAYS. A client that re-asserted its own measurement would win here within a frame,
    // which is the defect this claim exists to catch rather than a timing nicety.
    let held = settled_pane_dims(smoke, &mut daemon, &session);
    report.check(
        "the pin is not pulled back by the client that cannot show it whole",
        held == grown && !held.is_empty(),
    );

    // The un-pin, which is the only direction that separates a governed report from an absent one.
    if let Err(error) = smoke.cli(&["resize-window", "-t", &session, "-u"]) {
        eprintln!("      could not un-pin: {error}");
    }
    report.check(
        "un-pinning returns every pane to what this window measured",
        smoke
            .wait_for(|_| {
                let dims = pane_dims(&mut daemon, &session);
                (!dims.is_empty() && dims == solo).then_some(())
            })
            .is_ok(),
    );

    // Leave the file as this check found it, so a later one reads its own settings and not these.
    let _ = smoke.write_user_config("");
}

/// **`resize-window` from a KEY, through the shipped GUI binary (R331).**
///
/// The GUI half of `sprag-tui`'s pty gate, and R318's rule for why both exist: the keymap arm is
/// shared, the `perform` that carries it out is each front's own. Measured before this round,
/// `sprag bind-key R resize-window -a` was refused — *"a verb a keystroke could mean and sprag does
/// not bind it yet"* — so there was no key to press at either front.
///
/// # `-a` is the spelling that cannot be faked, and `-u` is what proves it was a PIN
///
/// The fold is resolved from the areas the DAEMON was told about, and the only client here is this
/// window — so the panes landing on what this window MEASURED says the request crossed, was resolved
/// there, and came back. The fixture pins somewhere else FIRST so that number is not already the
/// answer; without that the check would pass on a key that did nothing.
///
/// `-u` then hands it back, which is the direction that separates a stored decision from a window
/// that merely happened to follow its only client: under `manual` an un-pinned window defers to the
/// default policy and lands on the same rectangle, so what is asserted after the un-pin is that a
/// RELATIVE resize — which needs something to move — is refused.
fn check_the_resize_key_pins_this_windows_own_area(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the resize key", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the resize key", false);
        return;
    };
    let Some(measured) = window_size(&mut daemon, &session) else {
        report.check("this window reported an area to fold", false);
        return;
    };
    if smoke
        .write_user_config(
            "[options]\nwindow-size = \"manual\"\n\n[[bind]]\nkey = \"R\"\naction = \"resize-window -a\"\n\n[[bind]]\nkey = \"U\"\naction = \"resize-window -u\"\n",
        )
        .is_err()
    {
        report.check("the smoke binds the resize keys", false);
        return;
    }
    // The pane SLOTS are not re-packed as panes come and go, so the first docked one is read rather
    // than assumed to be zero — a check earlier in this run kills a pane, and slot 0 need not still
    // hold one by the time this runs.
    let docked = smoke
        .docked_panes()
        .ok()
        .and_then(|panes| panes.first().copied());
    let focused = docked.is_some_and(|pane| smoke.focus_pane(pane));
    report.check(
        &format!("a pane can be focused to drive the resize key ({docked:?})"),
        focused,
    );
    let (Some(pane), true) = (docked, focused) else {
        let _ = smoke.write_user_config("");
        return;
    };
    // OFF THE ANSWER FIRST. A window already on this client's area would satisfy the fold check
    // whether the key did anything at all — the vacuous-fixture shape R330's mutation pass caught.
    let elsewhere = (measured.0 + 17, measured.1 + 9);
    if let Err(error) = smoke.cli(&[
        "resize-window",
        "-t",
        &session,
        "-x",
        &elsewhere.0.to_string(),
        "-y",
        &elsewhere.1.to_string(),
    ]) {
        report.check("the smoke pins the window somewhere else first", false);
        eprintln!("      {error}");
        let _ = smoke.write_user_config("");
        return;
    }
    report.check(
        "the window starts somewhere no client reported",
        smoke
            .wait_for(|_| (window_size(&mut daemon, &session) == Some(elsewhere)).then_some(()))
            .is_ok(),
    );

    // PAST THE REPEAT WINDOW, on R308's hazard — inside it the prefix table is still live.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "R", false).is_ok();
    report.check("the GUI accepts `prefix R`", pressed);
    let folded =
        smoke.wait_for(|_| (window_size(&mut daemon, &session) == Some(measured)).then_some(()));
    report.check(
        &format!("`prefix R` pinned the window to this client's own area ({measured:?})"),
        folded.is_ok(),
    );

    // THE UN-PIN. Its discriminator is a RE-PIN first: with the window back on a rectangle no client
    // reported, only an un-pin can bring it back to this window's own area — `manual` defers to the
    // default policy when nothing is stored, which is this client's report.
    //
    // ⚠ The obvious discriminator does NOT work and measuring said so: a relative resize after the
    // un-pin still SUCCEEDS, because that deferral gives it a basis. A check written on the
    // reasonable-sounding version of this claim passed the pin half and hung on the un-pin.
    if let Err(error) = smoke.cli(&[
        "resize-window",
        "-t",
        &session,
        "-x",
        &elsewhere.0.to_string(),
        "-y",
        &elsewhere.1.to_string(),
    ]) {
        report.check("the smoke re-pins the window for the un-pin key", false);
        eprintln!("      {error}");
        let _ = smoke.write_user_config("");
        return;
    }
    if smoke
        .wait_for(|_| (window_size(&mut daemon, &session) == Some(elsewhere)).then_some(()))
        .is_err()
    {
        report.check("the window is pinned again before the un-pin key", false);
        let _ = smoke.write_user_config("");
        return;
    }
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let released = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "U", false).is_ok();
    report.check("the GUI accepts `prefix U`", released);
    let unpinned =
        smoke.wait_for(|_| (window_size(&mut daemon, &session) == Some(measured)).then_some(()));
    report.check(
        &format!("`prefix U` handed the window back to this client's own area ({unpinned:?})"),
        unpinned.is_ok(),
    );

    // ⚠ **THE THIRD OUTCOME, and the mutation pass is why it is here.** Under a policy that is not
    // `manual` the daemon STORES the size and lays nothing out over it — so the key changes nothing
    // on screen and is refused by nobody, which is indistinguishable from a key that is not bound
    // unless a sentence says so. Dropping `Report::pinned` from this front's arm left every check
    // above green; only this one turns it red.
    //
    // The keymap goes back in with the policy, because the client re-reads its config per keystroke
    // and a file holding only the option would unbind the key this check is about to press.
    if smoke
        .write_user_config(
            "[options]\nwindow-size = \"largest\"\n\n[[bind]]\nkey = \"R\"\naction = \"resize-window -x 90 -y 25\"\n",
        )
        .is_err()
    {
        report.check("the smoke flips the policy for the inert-pin sentence", false);
        let _ = smoke.write_user_config("");
        return;
    }
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let stored = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "R", false).is_ok();
    report.check(
        "the GUI accepts `prefix R` under a policy that ignores it",
        stored,
    );
    let said = smoke
        .wait_for_tag("sprag_message_strip")
        .ok()
        .and_then(|tags| {
            tags.get("sprag_message_strip")
                .map(|p| p.text.join("\u{1f}"))
        })
        .unwrap_or_default();
    report.check(
        &format!("a pin the policy IGNORES says so on this window ({said:?})"),
        said.contains("window-size is largest"),
    );

    // Leave the file as this check found it, so a later one reads its own settings and not these.
    let _ = smoke.write_user_config("");
}

/// **The WINDOW keys, through the shipped GUI binary** (R305) — `prefix c` creates a window and
/// `prefix n` walks the ring, judged against the DAEMON's own window list.
///
/// This is the GUI half of what `sprag-tui`'s pty test drives, and it exists because the two
/// frontends carry a bound action out in their OWN code: the keymap arm is shared, the `perform`
/// that runs it is not. A round that drove only the TUI would be inferring the GUI from a file it
/// does not use — the shape this project's register keeps flagging.
///
/// The prefix here is the SHIPPED one (`C-b`), because this check runs on whatever config the run
/// leaves in force and the keymap gate above deliberately ends on an empty file.
///
/// **What it leaves, stated** (item 15's hazard, on the round that adds a check): one extra window,
/// and the session on its FIRST window rather than the one it found current. Every later check
/// reads the window it is on rather than a fixed name, so that is survivable — and saying it here
/// is what stops the next check being written against an inheritance nobody wrote down.
fn check_the_window_keys_reach_the_daemon(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the window keys", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the window keys", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the window keys", false);
        return;
    };
    let before = windows_of(&mut daemon, &session);
    report.check(
        &format!("the session has windows to start from ({before:?})"),
        !before.is_empty(),
    );

    // `prefix c` — the key that did nothing at all before this round.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "c", false).is_ok();
    report.check("the GUI accepts `prefix c`", pressed);
    let grown = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        (now.len() > before.len()).then_some(now)
    });
    report.check(
        &format!("`prefix c` created a window on the daemon ({grown:?})"),
        grown.is_ok(),
    );
    let Ok(grown) = grown else { return };
    report.check(
        "...and the daemon selected it, which is what a client then projects",
        grown.last().is_some_and(|(_, current)| *current),
    );

    // `prefix n` — the RING, walked by the daemon. From the last window it WRAPS onto the first,
    // which is the half a client-side walk would be free to get wrong.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "n", false).is_ok();
    report.check("the GUI accepts `prefix n`", pressed);
    let walked = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        now.first()
            .is_some_and(|(_, current)| *current)
            .then_some(now)
    });
    report.check(
        &format!("`prefix n` wrapped onto the session's first window ({walked:?})"),
        walked.is_ok(),
    );
}

/// **`prefix !` takes the focused pane into a window of its own, through the shipped GUI binary
/// (R323).**
///
/// The GUI half of `sprag-tui`'s pty gate, and the reason both exist is R318's: a claim is driven at
/// every front that has one, because the two clients perform a bound action through their own code.
/// Measured before this round, `break-pane` was a verb the CLI dispatched and `bind-key` refused —
/// so there was no key to press at either front.
///
/// **A SHIFTED CHARACTER**, which is its own small claim on this front: winit delivers `!` with the
/// shift bit set, and `KeySpec::matches` masks shift off a character key (R306). A client that
/// compared the modifier exactly would leave this key dead while `prefix c` worked.
///
/// Judged against the DAEMON's window list, not against pixels: which window holds which pane is a
/// fact only the daemon has, and this client's tab strip is its projection of it.
fn check_the_break_key_gives_a_pane_a_window_of_its_own(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the break key", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the break key", false);
        return;
    };
    let Ok(panes) = smoke.pane_count() else {
        report.check("the smoke can count panes for the break key", false);
        return;
    };
    // THE FIXTURE IS THE CLAIM'S PRECONDITION, asserted rather than assumed: with one pane in the
    // window the break has no observable half, so a green check would mean nothing.
    report.check(
        &format!("the window holds more than one pane to break out of ({panes})"),
        panes > 1,
    );
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the break key", false);
        return;
    };
    let before = windows_of(&mut daemon, &session);
    // PAST THE REPEAT WINDOW, on R308's hazard — see the guarded kill's own statement of it.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "!", false).is_ok();
    report.check("the GUI accepts `prefix !`", pressed);
    let grown = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        (now.len() > before.len()).then_some(now)
    });
    report.check(
        &format!("`prefix !` gave the pane a window of its own ({grown:?})"),
        grown.is_ok(),
    );
    let Ok(grown) = grown else { return };
    report.check(
        "...and the daemon selected it, which is what this client then projects",
        grown.last().is_some_and(|(_, current)| *current),
    );
    // THE PANE WENT WITH IT, and this client followed the daemon onto the window it made: what is
    // docked now is ONE pane, where a client that merely created an empty window would still be
    // projecting the several it started with.
    let alone = smoke.wait_for(|s| s.pane_count().ok().filter(|now| *now == 1));
    report.check(
        &format!("the broken-out pane is alone in the window it made ({alone:?})"),
        alone.is_ok(),
    );
}

/// **`prefix )` / `prefix (` / `prefix L` move this CLIENT to another SESSION, through the shipped
/// GUI binary (R314).**
///
/// The GUI half of what `sprag-tui`'s pty test drives. It matters most here for a reason no other
/// key check has: **the three session chords used to be this binary's PRIVATE table** — hard-coded
/// `Ctrl+Shift+{L,PageUp,PageDown}` that `sprag list-keys` could not name and a config could not
/// unbind — and R314 deleted them in favour of these bindings. So this check is what says the
/// capability survived the move, in the client that had it.
///
/// Judged against the DAEMON's per-session VIEWER BADGE, not against pixels: "this client is now
/// attached to that session" is a fact only the daemon holds, and the badge is where it publishes
/// it. Reading the strip instead would test this client's paint and not the switch.
///
/// **What it leaves, stated**: the client back on the session it started from. The last press is
/// `prefix L`, which returns it, and every later check reads the session it is on rather than a
/// fixed name.
fn check_the_session_keys_move_this_client(smoke: &mut Smoke, report: &mut Report) {
    let Some(home) = smoke.attached_session() else {
        report.check("the window names its session for the session keys", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the session keys", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the session keys", false);
        return;
    };
    // A SECOND session, or the ring wraps onto the one we are on and every assertion below passes
    // without discriminating — the vacuous-fixture shape this project has now caught six times.
    let made = daemon.call(
        "scene/invoke",
        json!({ "path": "/sprag_mux/external/new_session", "args": { "name": "smoke-elsewhere" } }),
    );
    report.check(
        &format!("a second session exists for the ring to reach ({made:?})"),
        made.is_ok(),
    );
    let listed = sessions_of(&mut daemon);
    report.check(
        &format!("the daemon lists more than one session ({listed:?})"),
        listed.len() > 1,
    );
    if listed.len() < 2 {
        return;
    }
    report.check(
        &format!("this client is counted on the session it is on ({home})"),
        attached_to(&mut daemon, &home) > 0,
    );

    // `prefix )` — one step along the ring. `)` is a SHIFTED character, R306's class: winit reports
    // it with the shift flag where a pty reports it without one, so this press is also the standing
    // check that the masking fix still holds one verb further on.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, ")", false).is_ok();
    report.check("the GUI accepts `prefix )`", pressed);
    let moved = smoke.wait_for(|s| {
        let _ = s;
        let now = attached_to(&mut daemon, &home);
        (now == 0).then_some(now)
    });
    report.check(
        &format!("`prefix )` moved this client OFF the session it was on ({moved:?})"),
        moved.is_ok(),
    );
    if moved.is_err() {
        return;
    }
    // ...and onto one the daemon can name. Read as a SET rather than as a name: which session the
    // ring lands on is the daemon's business, and pinning it here would make this check fail for a
    // reordering that is none of its concern.
    let landed = smoke.wait_for(|s| {
        let _ = s;
        let where_now: Vec<String> = sessions_of(&mut daemon)
            .into_iter()
            .filter(|name| attached_to(&mut daemon, name) > 0)
            .collect();
        (where_now.len() == 1 && where_now[0] != home).then_some(where_now)
    });
    report.check(
        &format!("...and onto a DIFFERENT one, which the daemon counts ({landed:?})"),
        landed.is_ok(),
    );

    // `prefix L` — back to the session VISITED before this one. A key that merely stepped again
    // would satisfy nothing here on a two-session ring, so the assertion is the NAME: `home`.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "L", false).is_ok();
    report.check("the GUI accepts `prefix L`", pressed);
    let back = smoke.wait_for(|s| {
        let _ = s;
        let now = attached_to(&mut daemon, &home);
        (now > 0).then_some(now)
    });
    report.check(
        &format!("`prefix L` brought this client back to {home} ({back:?})"),
        back.is_ok(),
    );
}

/// **`prefix s` opens the CHOOSER through the shipped GUI binary, and the row a person picks is
/// where this client lands** (R315).
///
/// The GUI half of what `sprag-tui`'s pty test drives, and it exists for the reason every one of
/// these pairs does: the ask is decided once in `sprag_host::chooser`, and the SURFACE that paints
/// it is each frontend's own code — a modal here, an overlay there. A shared decision proves
/// nothing about a panel this binary draws.
///
/// **THE FIXTURE MAKES THE TWO READINGS DISAGREE.** A second session is made and the query is typed
/// until only ITS row can be picked, so a chooser that committed whatever the cursor started on —
/// which is the session this client is already viewing — fails here. The DAEMON's viewer badge is
/// what is judged, never the paint: a client that drew a beautiful list and moved nobody would
/// satisfy any assertion made on its own tree.
///
/// **What it leaves, stated**: this client back on the session it started from, and the session it
/// visited still standing. It runs after the session-key check, which already leaves a second
/// session for it to reach.
fn check_the_chooser_opens_and_a_picked_row_moves_this_client(
    smoke: &mut Smoke,
    report: &mut Report,
) {
    let Some(home) = smoke.attached_session() else {
        report.check("the window names its session for the chooser", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the chooser", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to open the chooser", false);
        return;
    };
    // A SECOND session to pick, made here rather than inherited: this check must discriminate on
    // its own, and a fixture that depended on an earlier check's leftovers would pass or fail for
    // that check's reasons.
    let made = daemon.call(
        "scene/invoke",
        json!({ "path": "/sprag_mux/external/new_session", "args": { "name": "smoke-chosen" } }),
    );
    report.check(
        &format!("a session exists for the chooser to offer ({made:?})"),
        made.is_ok(),
    );
    report.check(
        &format!("this client starts on the session it is viewing ({home})"),
        attached_to(&mut daemon, &home) > 0,
    );

    // PAST THE REPEAT WINDOW FIRST — the key-table check's own note, and the reason is sharper
    // here: the character this check presses is `s`, and inside an open repeat window the prefix
    // table is still live, so `C-b` would be a self-send and the `s` behind it would arm the
    // chooser at the wrong moment. R308 registered this hazard; R315's own `sprag-tui` test was
    // caught by it.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "s", false).is_ok();
    report.check("the GUI accepts `prefix s`", pressed);
    let opened = smoke.wait_for_tag("sprag_chooser_panel");
    report.check(
        &format!("`prefix s` opened the chooser ({opened:?})"),
        opened.is_ok(),
    );
    if opened.is_err() {
        return;
    }

    // WHAT IT PAINTED, read off the strings the panel actually put on the screen — not off its
    // accessible value and not off a tree this process could build for itself. The claim is that a
    // shipped binary showed a person what is there, and only the painted tree can say that.
    let text = opened
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_chooser_panel"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("the panel painted its rows ({} chars)", text.len()),
        !text.is_empty(),
    );
    report.check(
        "...and it names a session this client is NOT on, which a name prompt could not",
        text.contains("smoke-chosen"),
    );
    report.check(
        "...and says how big it is, so two rows can be told apart",
        text.contains("pane"),
    );

    // TYPE TO NARROW. Every character is the panel's — the pane behind it must not see them, which
    // is judged on the DAEMON's pane list rather than on this client's paint.
    let panes_before = daemon_panes(&mut daemon, &home);
    for key in ["s", "m", "o", "k", "e", "-", "c"] {
        let _ = smoke.press(pane, key, false);
    }
    let panes_after = daemon_panes(&mut daemon, &home);
    report.check(
        &format!("no key reaches the panes behind it ({panes_before:?} -> {panes_after:?})"),
        panes_before == panes_after,
    );

    // ...AND ENTER GOES THERE. Judged on the daemon's badges: OFF the session this client was on,
    // and ON the one whose row survived the query.
    let _ = smoke.press(pane, "Enter", false);
    let landed = smoke.wait_for(|s| {
        let _ = s;
        (attached_to(&mut daemon, "smoke-chosen") > 0 && attached_to(&mut daemon, &home) == 0)
            .then_some(())
    });
    report.check(
        &format!("a picked row moved this client to the session it named ({landed:?})"),
        landed.is_ok(),
    );
    let gone = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_chooser_panel")).then_some(())
    });
    report.check(
        &format!("...and the panel is gone, so the panes have the keyboard ({gone:?})"),
        gone.is_ok(),
    );

    // BACK, so what follows inherits the session this check was handed. `prefix L` is the verb for
    // exactly this and is already proved by the check above it.
    let _ = smoke.press(pane, "b", true);
    let _ = smoke.press(pane, "L", false);
    let back = smoke.wait_for(|s| {
        let _ = s;
        (attached_to(&mut daemon, &home) > 0).then_some(())
    });
    report.check(
        &format!("...and this check leaves the client where it found it ({back:?})"),
        back.is_ok(),
    );
}

/// **A key that finds nothing SAYS SO, through the shipped GUI binary** (R316).
///
/// The GUI half of what `sprag-tui`'s pty test drives, and the pair is the whole point: the
/// sentence is decided once in `sprag_host::report`, and the SURFACE that shows it is each
/// frontend's own — a reserved status row there, an overlay strip here. A shared decision proves
/// nothing about a strip this binary draws.
///
/// **THE FIXTURE MAKES THE TWO READINGS DISAGREE.** One key is bound to a session that exists and
/// one to a session that does not, in the same config, and both are pressed: the first must leave
/// the client silent (and MOVED), the second must paint the sentence. A strip that appeared on
/// every keystroke, and one that never appeared, each fail on one of the two.
///
/// The strip's TAG is read out of the painted tree rather than out of this process's own state,
/// which is what makes it a claim about pixels; and the DAEMON's viewer badge is what says the good
/// key worked, never the paint.
///
/// **What it leaves, stated**: the shipped keymap back in force, and this client on the session it
/// started from.
fn check_a_key_that_finds_nothing_says_so_on_the_screen(smoke: &mut Smoke, report: &mut Report) {
    let Some(home) = smoke.attached_session() else {
        report.check("the window names its session for the report", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the report", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to press a reporting key", false);
        return;
    };
    // A session the GOOD key can reach, made here so this check discriminates on its own.
    let made = daemon.call(
        "scene/invoke",
        json!({ "path": "/sprag_mux/external/new_session", "args": { "name": "smoke-report" } }),
    );
    report.check(
        &format!("a session exists for the good key to reach ({made:?})"),
        made.is_ok(),
    );
    // TWO bindings in one file, so the pair is pressed against one table.
    let wrote = smoke.write_user_config(
        "[[bind]]\nkey = \"y\"\naction = \"switch-client -t smoke-report\"\n\n\
         [[bind]]\nkey = \"g\"\naction = \"switch-client -t no-such-session\"\n",
    );
    report.check(
        &format!("the two bindings are written ({wrote:?})"),
        wrote.is_ok(),
    );

    // PAST THE REPEAT WINDOW FIRST — R308's hazard, which R315 was bitten by: inside an open window
    // the prefix table is still live, so the first character of this chord would self-send.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);

    // THE GOOD KEY, pressed first so the silence below is measured on a client that was WORKING.
    let good = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "y", false).is_ok();
    report.check("the GUI accepts the good binding", good);
    let moved = smoke.wait_for(|s| {
        let _ = s;
        (attached_to(&mut daemon, "smoke-report") > 0).then_some(())
    });
    report.check(
        &format!("a key naming a session that EXISTS moves this client ({moved:?})"),
        moved.is_ok(),
    );
    let quiet = smoke
        .tags()
        .map(|tags| !tags.contains_key("sprag_message_strip"))
        .unwrap_or(false);
    report.check(
        "...and says nothing, because the session tabs already show where it went",
        quiet,
    );

    // THE BAD KEY. The strip must appear, and it must NAME the session that is not there.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let bad = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "g", false).is_ok();
    report.check("the GUI accepts the bad binding", bad);
    let shown = smoke.wait_for_tag("sprag_message_strip");
    report.check(
        &format!("a key naming a session that does NOT exist raises the strip ({shown:?})"),
        shown.is_ok(),
    );
    let said = shown
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_message_strip"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("...and the strip NAMES what is not there ({said:?})"),
        said.contains("no session called") && said.contains("no-such-session"),
    );
    report.check(
        "...and it says SESSION, not the action's grouping subject",
        !said.contains("no client called"),
    );
    report.check(
        &format!("...and the refused switch moved nobody ({said:?})"),
        attached_to(&mut daemon, "smoke-report") > 0,
    );

    // IT GOES AWAY ON ITS OWN. A pinion window repaints on damage, so the strip's expiry rides on a
    // wake this client schedules for itself — the one timer it has. Nothing is pressed from here,
    // which is the state a clearing that rode on the next event would survive forever.
    let cleared = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!(
            "...and the strip clears on its own deadline, with no key to prompt it ({cleared:?})"
        ),
        cleared.is_ok(),
    );

    // THE DIRECTIONAL EDGE, on the same client and through the same strip: `prefix ArrowLeft` on
    // the leftmost pane has nowhere to go, and the arm that says so is `perform`'s — this front's
    // own code, which the terminal front's live fixture cannot speak for.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let edged =
        smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "ArrowLeft", false).is_ok();
    report.check("the GUI accepts `prefix ArrowLeft`", edged);
    let at_edge = smoke.wait_for_tag("sprag_message_strip");
    let said_edge = at_edge
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_message_strip"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("a directional key at the edge says so here too ({said_edge:?})"),
        said_edge.contains("select-pane -L: nowhere to go"),
    );

    // ----- R317: a message somebody ELSE sent, on the same strip -----
    //
    // The strip so far has only ever carried what THIS client's own keyboard did. These drive the
    // other half: a `display_message` sent over the wire — the CLI's and an agent's path — reaching
    // the shipped GUI binary, and an ALERT that does not go away on a clock.
    let cleared_before = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("the strip is empty before the routed message ({cleared_before:?})"),
        cleared_before.is_ok(),
    );
    let sent = daemon.call(
        "scene/invoke",
        json!({
            "path": "/sprag_mux/external/display_message",
            "session": "smoke-report",
            "args": { "text": "the deploy finished", "severity": "note" },
        }),
    );
    report.check(
        &format!("the daemon accepts a message for this client's session ({sent:?})"),
        sent.is_ok(),
    );
    // ...AND SAYS WHO. The delivery is a list, and this client must be in it — a `{clients: []}`
    // here would mean the daemon believed nobody was attached while a window was on screen.
    let delivered = sent
        .as_ref()
        .ok()
        .and_then(|answer| answer["clients"].as_array().cloned())
        .unwrap_or_default();
    report.check(
        &format!("...and names the client it reached ({delivered:?})"),
        delivered.len() == 1,
    );
    let routed = smoke.wait_for_tag("sprag_message_strip");
    let said_routed = routed
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_message_strip"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("a message SENT BY ANOTHER PROCESS reaches this window ({said_routed:?})"),
        said_routed.contains("the deploy finished"),
    );
    // A NOTE clears itself, with nothing pressed — the CONTROL for the alert below, which must not.
    let note_cleared = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("...and a NOTE clears on its own deadline ({note_cleared:?})"),
        note_cleared.is_ok(),
    );

    // THE ALERT. No deadline: it waits for a person, which is the property no rival surface has.
    let alerted = daemon.call(
        "scene/invoke",
        json!({
            "path": "/sprag_mux/external/display_message",
            "session": "smoke-report",
            "args": { "text": "the deploy needs you", "severity": "alert" },
        }),
    );
    report.check(
        &format!("the daemon accepts an ALERT ({alerted:?})"),
        alerted.is_ok(),
    );
    let raised = smoke.wait_for_tag("sprag_message_strip");
    report.check(
        &format!("the alert raises the strip ({raised:?})"),
        raised
            .as_ref()
            .ok()
            .and_then(|tags| tags.get("sprag_message_strip"))
            .map(|painted| painted.text.join("\u{1f}"))
            .unwrap_or_default()
            .contains("the deploy needs you"),
    );
    // Sampled well past the note's whole lifetime — the note above cleared inside this window, so a
    // client that treated every message alike is caught rather than merely raced.
    let mut still_up = true;
    for _ in 0..20 {
        std::thread::sleep(POLL);
        still_up = smoke
            .tags()
            .map(|tags| tags.contains_key("sprag_message_strip"))
            .unwrap_or(false);
        if !still_up {
            break;
        }
    }
    report.check(
        "an ALERT does not expire on a clock, where the note beside it did",
        still_up,
    );
    // ...and a keystroke takes it away. `prefix q`: `q` is bound to NOTHING, so the client swallows
    // it — it reaches no pane and runs no action — which makes the ACKNOWLEDGEMENT the only thing
    // that could have cleared the strip. It also leaves the prefix consumed, so the cleanup below
    // starts from the steady state (the first draft pressed the prefix ALONE and the cleanup's own
    // chord was then eaten as a `send-prefix`, which is what the failing run said).
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "q", false).is_ok();
    report.check("the GUI accepts the acknowledging keystroke", pressed);
    let acknowledged = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("...and a keystroke is what clears it ({acknowledged:?})"),
        acknowledged.is_ok(),
    );

    // ----- R318: a message this window's OWN PANE raised, on the same strip -----
    //
    // Measured at `3114923`: `sprag-gui` showed a DOT on the pane title and DROPPED the words. The
    // dot is still right (it persists until the pane is viewed); what was missing is that the
    // sentence reached nobody. This drives it through the shipped binary, from a real child.
    let quiet_first = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("the strip is empty before the pane raises anything ({quiet_first:?})"),
        quiet_first.is_ok(),
    );
    // The CHILD raises it: a `printf` typed into the pane's own shell, which is how a build script
    // or a test runner does it. Typed through the DAEMON's input external — the client's socket
    // cannot reach a pane's input, which is the seam `Smoke::daemon` exists for.
    // Named apart from the focused SLOT above: this is a daemon PANE ID and that one is this
    // client's slot index. Both were called `pane` and the second shadowed the first, which the
    // keyboard lines at the end of this check silently inherited (R331).
    let raising = daemon_panes(&mut daemon, "smoke-report")
        .first()
        .copied()
        .unwrap_or(0);
    let typed = daemon.call(
        "scene/invoke",
        json!({
            "path": sprag_host::wire::pane_input_path(u64::from(raising), sprag_host::wire::TEXT_ACTION),
            "session": "smoke-report",
            "args": { "text": "printf '\\033]9;build finished: 3 errors\\007'\r" },
        }),
    );
    report.check(
        &format!("the pane's shell accepts the notifying command ({typed:?})"),
        typed.is_ok(),
    );
    // The DAEMON's own view first, which separates "the child never raised one" from "nothing
    // delivered it" — the two failures that look identical from an empty strip.
    let latched = smoke.wait_for(|_| {
        let panes = daemon
            .call(
                "scene/query",
                json!({ "session": "smoke-report", "path": sprag_host::wire::mux_action_path(sprag_host::wire::PANES_SLOT) }),
            )
            .ok()?;
        let seen = panes
            .as_array()?
            .iter()
            .any(|row| row["notification"]["body"].as_str() == Some("build finished: 3 errors"));
        seen.then_some(())
    });
    report.check(
        &format!("the DAEMON latched the child's notification ({latched:?})"),
        latched.is_ok(),
    );
    let from_the_pane = smoke.wait_for_tag("sprag_message_strip");
    let said_pane = from_the_pane
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_message_strip"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("a pane CHILD's own notification reaches this window ({said_pane:?})"),
        said_pane.contains("build finished: 3 errors"),
    );
    report.check(
        &format!("...and the strip NAMES the pane it came from ({said_pane:?})"),
        said_pane.contains(&format!("pane {raising}")),
    );

    // WHAT IT LEAVES: the shipped table back, and this client where it started.
    let restored = smoke.write_user_config("");
    report.check(
        &format!("the shipped keymap is put back ({restored:?})"),
        restored.is_ok(),
    );
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let _ = smoke.press(pane, "b", true);
    let _ = smoke.press(pane, "L", false);
    let back = smoke.wait_for(|s| {
        let _ = s;
        (attached_to(&mut daemon, &home) > 0).then_some(())
    });
    report.check(
        &format!("...and this check leaves the client where it found it ({back:?})"),
        back.is_ok(),
    );
}

/// **A message reaches the person after they have left the window** — the windowed half of R319.
///
/// # What was measured before this existed
///
/// `sprag-tui` copies a message out to its host terminal as `OSC 9` when the person is not looking.
/// `sprag-gui` did nothing at all: `notify-outward` was a setting a windowed client read and never
/// acted on, so a message delivered to a blurred window was painted onto a strip nobody could see —
/// R318's *"every layer carried it and nothing was obliged to read it"*, one front further along.
/// This check drives that claim through the SHIPPED binary against a recorder standing in for the
/// desktop's own notifier, so what it reads is the argv the product actually built.
fn check_a_message_follows_the_person_out_of_the_window(smoke: &mut Smoke, report: &mut Report) {
    let Some(home) = smoke.attached_session() else {
        report.check("the window names its session for the outward copy", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the outward copy", false);
        return;
    };
    let state = smoke.state.clone();
    let client = smoke.gui.id();

    // THE POLICY IS WRITTEN, NOT INHERITED. R318 and R319 both shipped gates that read whatever
    // `notify-outward` happened to be in force; here the isolation holds (every child gets its own
    // `XDG_CONFIG_HOME`) but the VALUE would still be whatever the check before this one left, and
    // a claim about the default policy that silently becomes a claim about somebody else's config
    // is the same defect one layer in. The isolation was PROVED the way that lesson says: putting
    // `off` in this file turns the assertions below RED.
    let policy = smoke.write_user_config("[options]\nnotify-outward = \"unfocused\"\n");
    report.check(
        &format!("the outward policy under test is written, not inherited ({policy:?})"),
        policy.is_ok(),
    );

    // THE CONTROL FIRST, and it is the half that has to be able to fail: a message delivered while
    // the person is LOOKING must reach the strip and nothing else. Put first so a notifier that
    // fired for everything is caught here rather than passing the interesting half by accident.
    let before = notify_calls_by(&state, client).len();
    let focused = smoke.call("scene/window_focus", json!({ "focused": true }));
    report.check(
        &format!("the window can be told it holds OS focus ({focused:?})"),
        focused.is_ok(),
    );
    let seen = daemon.call(
        "scene/invoke",
        json!({
            "path": "/sprag_mux/external/display_message",
            "session": home,
            "args": { "text": "a message the person can see", "severity": "note" },
        }),
    );
    report.check(
        &format!("the daemon accepts a message for a WATCHED window ({seen:?})"),
        seen.is_ok(),
    );
    let painted = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        let strip = tags.get("sprag_message_strip")?;
        strip
            .text
            .join("\u{1f}")
            .contains("a message the person can see")
            .then_some(())
    });
    report.check(
        &format!("...and it reaches the strip ({painted:?})"),
        painted.is_ok(),
    );
    // Sampled AFTER the strip proved the delivery landed, so this is "nothing was sent", not
    // "nothing had been sent yet".
    report.check(
        &format!(
            "a message a person can READ is not also thrown at their desktop ({:?})",
            notify_calls_by(&state, client).len() - before,
        ),
        notify_calls_by(&state, client).len() == before,
    );

    // THE CLAIM. The person leaves: the WM takes focus off every window this client owns.
    let blurred = smoke.call("scene/window_focus", json!({ "focused": false }));
    report.check(
        &format!("the window can be told it lost OS focus ({blurred:?})"),
        blurred.is_ok(),
    );
    let sent = daemon.call(
        "scene/invoke",
        json!({
            "path": "/sprag_mux/external/display_message",
            "session": home,
            "args": { "text": "the deploy needs you", "severity": "alert" },
        }),
    );
    report.check(
        &format!("the daemon accepts a message for an UNWATCHED window ({sent:?})"),
        sent.is_ok(),
    );
    let followed = smoke.wait_for(|s| {
        let _ = s;
        (notify_calls_by(&state, client).len() > before).then_some(())
    });
    let argv = notify_calls_by(&state, client)
        .last()
        .cloned()
        .unwrap_or_default();
    report.check(
        &format!("a message reaches the person's DESKTOP once they have left ({argv:?})"),
        followed.is_ok(),
    );
    // The words, the session it came from, and the URGENCY — the three things a desktop
    // notification has to carry for it to be worth more than a beep.
    let joined = argv.join(" ");
    report.check(
        &format!("...carrying the words the strip would have shown ({joined:?})"),
        joined.contains("the deploy needs you"),
    );
    report.check(
        &format!("...naming the session it came from ({joined:?})"),
        joined.contains(&home),
    );
    report.check(
        &format!("...and an ALERT asks for the CRITICAL urgency ({joined:?})"),
        argv.windows(2)
            .any(|pair| pair[0] == "-u" && pair[1] == "critical"),
    );

    // ...AND THE PANE'S OWN CHILD REACHES THEM TOO, which is the case this whole front exists for:
    // a build that finishes while somebody is in their browser. It arrives through R318's route (a
    // child's `OSC 9` becomes an `Announcement` on the same per-client mailbox) so the seam is
    // shared with the message above — but "shared with something that is tested" is not a test, and
    // this is the half a person actually meets.
    //
    // The alert above is acknowledged FIRST: the mailbox holds one message per client, and a
    // sentence that waits for a keystroke would otherwise still be the one on the strip when this
    // one is asserted.
    let _ = smoke.press(0, "b", true);
    let _ = smoke.press(0, "q", false);
    let cleared_first = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("the alert is acknowledged before the child speaks ({cleared_first:?})"),
        cleared_first.is_ok(),
    );
    // A keystroke is only allowed to move the STRIP, never the window manager — if pressing a key
    // had re-focused this window the reading below would be about a person who came back.
    let still_away = smoke.call("scene/window_focus", json!({ "focused": false }));
    report.check(
        &format!("the person is still away after the acknowledgement ({still_away:?})"),
        still_away.is_ok(),
    );
    let child_from = notify_calls_by(&state, client).len();
    let pane = daemon_panes(&mut daemon, &home)
        .first()
        .copied()
        .unwrap_or(0);
    let raised = daemon.call(
        "scene/invoke",
        json!({
            "path": sprag_host::wire::pane_input_path(u64::from(pane), sprag_host::wire::TEXT_ACTION),
            "session": home,
            // `\\033` / `\\007` are LITERAL backslash escapes for the shell's own `printf`, not Rust
            // ones: written singly, Rust reads `\0` as a NUL and types a nul byte plus `33]9;` into
            // the pane, which raises nothing. The first draft did exactly that, and the strip check
            // above is what said so — a bare "the desktop got nothing" had blamed the product.
            "args": { "text": "printf '\\033]9;the build finished\\007'\r" },
        }),
    );
    report.check(
        &format!("the pane's shell accepts the notifying command ({raised:?})"),
        raised.is_ok(),
    );
    // THE STRIP FIRST, so this reading can say WHICH failure it is: a child's words that never
    // reached the client at all and a client that received them and forwarded nothing are opposite
    // diagnoses, and a bare "the desktop got nothing" cannot tell them apart.
    let on_strip = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        let strip = tags.get("sprag_message_strip")?;
        strip
            .text
            .join("\u{1f}")
            .contains("the build finished")
            .then_some(())
    });
    report.check(
        &format!("the child's words reached this client at all ({on_strip:?})"),
        on_strip.is_ok(),
    );
    let chased = smoke.wait_for(|s| {
        let _ = s;
        (notify_calls_by(&state, client).len() > child_from).then_some(())
    });
    let child_argv = notify_calls_by(&state, client)
        .last()
        .cloned()
        .unwrap_or_default();
    report.check(
        &format!("a PANE CHILD's notification follows the person out too ({child_argv:?})"),
        chased.is_ok() && child_argv.join(" ").contains("the build finished"),
    );

    // THE SECOND CONTROL, and the one that makes this a test of the POLICY rather than of the
    // window manager: the same blurred window under `notify-outward = off` sends NOTHING. Without
    // it, a client that forwarded on every unfocused message regardless of the setting would pass
    // everything above — and the setting is the whole reason this is a policy and not a feature.
    let silenced = smoke.write_user_config("[options]\nnotify-outward = \"off\"\n");
    report.check(
        &format!("the policy can be turned off without restarting the client ({silenced:?})"),
        silenced.is_ok(),
    );
    let quiet_from = notify_calls_by(&state, client).len();
    let refused = daemon.call(
        "scene/invoke",
        json!({
            "path": "/sprag_mux/external/display_message",
            "session": home,
            "args": { "text": "a message nobody asked to be chased with", "severity": "alert" },
        }),
    );
    report.check(
        &format!("the daemon accepts a message under the OFF policy ({refused:?})"),
        refused.is_ok(),
    );
    // Waited for on the STRIP, so the sample below is taken after the delivery has demonstrably
    // landed — otherwise "nothing was sent" would be indistinguishable from "not yet".
    let landed = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        let strip = tags.get("sprag_message_strip")?;
        strip
            .text
            .join("\u{1f}")
            .contains("a message nobody asked to be chased with")
            .then_some(())
    });
    report.check(
        &format!("...and it still reaches the strip ({landed:?})"),
        landed.is_ok(),
    );
    report.check(
        &format!(
            "an OFF policy chases nobody, on the same blurred window ({} calls)",
            notify_calls_by(&state, client).len() - quiet_from,
        ),
        notify_calls_by(&state, client).len() == quiet_from,
    );

    // WHAT IT LEAVES: the shipped config back, the window focused again, and the strip clear —
    // every later check reads all three.
    let put_back = smoke.write_user_config("");
    report.check(
        &format!("the shipped config is put back ({put_back:?})"),
        put_back.is_ok(),
    );
    let restored = smoke.call("scene/window_focus", json!({ "focused": true }));
    report.check(
        &format!("the window is left holding focus ({restored:?})"),
        restored.is_ok(),
    );
    // ...and the OFF control's own alert acknowledged, so the strip is clear for whatever runs
    // next. `prefix q` is bound to nothing, so the acknowledgement is the only thing it can do.
    let _ = smoke.press(0, "b", true);
    let _ = smoke.press(0, "q", false);
    let cleared = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_message_strip")).then_some(())
    });
    report.check(
        &format!("...and the strip is clear before the next check ({cleared:?})"),
        cleared.is_ok(),
    );
}

/// **`prefix >` and `prefix <` move a WINDOW's place, through the shipped GUI binary.**
///
/// The GUI half of what `sprag-tui`'s pty test drives, and it exists for the reason the check above
/// states: the keymap arm is shared, the `perform` that runs it is each frontend's own code. This
/// client is also the one that DRAWS the order — the window strip — so it is the client where a
/// wrong answer would be visible to a user, and the only one where "the order moved" is a claim
/// about something on screen.
///
/// Judged against the DAEMON's window list rather than against painted pixels, deliberately: the
/// strip is a projection of that list and asserting on it would test this client's paint twice
/// while testing the verb not at all.
///
/// **What it leaves, stated**: the window order as it found it. Both presses are made and the
/// second undoes the first, so a later check inherits the arrangement the window-key check left.
fn check_the_order_keys_move_a_window_on_the_daemon(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the order keys", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the order keys", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the order keys", false);
        return;
    };
    let before = windows_of(&mut daemon, &session);
    // TWO windows at least, or a move has nowhere to go and every assertion below is vacuous —
    // which is the shape this project has caught five times, so it is checked rather than assumed.
    report.check(
        &format!("the session has more than one window to reorder ({before:?})"),
        before.len() > 1,
    );
    if before.len() < 2 {
        return;
    }
    let names = |list: &[(String, bool)]| -> Vec<String> {
        list.iter().map(|(name, _)| name.clone()).collect()
    };
    let on = |list: &[(String, bool)]| -> Option<String> {
        list.iter()
            .find(|(_, current)| *current)
            .map(|(name, _)| name.clone())
    };
    let was = names(&before);
    let sitting = on(&before);

    // `prefix >` — one place toward the back. `>` is a SHIFTED character, which is the class R306
    // measured `prefix %` failing on in this client on a real keyboard: winit reports it with the
    // shift flag where a pty reports it without one. So this press is also the standing check that
    // the fix held.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, ">", false).is_ok();
    report.check("the GUI accepts `prefix >`", pressed);
    let moved = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        (names(&now) != was).then_some(now)
    });
    report.check(
        &format!("`prefix >` moved a window on the daemon ({moved:?})"),
        moved.is_ok(),
    );
    let Ok(moved) = moved else { return };
    report.check(
        "...and moved the WINDOW, not the user: the session sits where it did",
        on(&moved) == sitting,
    );
    report.check(
        "...and moved it rather than adding or dropping one",
        moved.len() == before.len(),
    );

    // `prefix <` — the other way, which must put the order back. A key that did the same thing as
    // its twin would pass every assertion above and fail this one.
    // ...and the STRIP the user looks at follows. Read off the tabs' own painted text, not asked
    // of the daemon: this client is the one that DRAWS the order, so "the daemon moved it" and "the
    // user can see it moved" are two claims and only the second is about this binary.
    let painted = smoke.wait_for(|s| {
        let tabs = s.tabs().ok()?;
        (tabs == names(&moved)).then_some(tabs)
    });
    report.check(
        &format!("the window strip PAINTS the new order ({painted:?})"),
        painted.is_ok(),
    );

    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "<", false).is_ok();
    report.check("the GUI accepts `prefix <`", pressed);
    let back = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        (names(&now) == was).then_some(now)
    });
    report.check(
        &format!("`prefix <` put the order back ({back:?})"),
        back.is_ok(),
    );
}

/// **`prefix C-Left` moves a real boundary, through the shipped GUI binary.**
///
/// The keymap ARM is shared between the frontends, but the `perform` that runs it is each one's own
/// code — so a round that drove only `sprag-tui`'s pty test would be inferring this client from a
/// file it does not use. R305's finding, applied on the round that adds the verb rather than left
/// for an audit to register.
///
/// It is judged on the DAEMON's pane widths, not on this window's tiles: a client that re-tiled its
/// own surface while telling the daemon nothing would paint a picture that looks right over two
/// children still running at the old width. And it is judged on a DIFFERENCE rather than on an
/// absolute number, because the window here is whatever size the headless surface came up at.
///
/// The two directions are both driven, from the same pane, which is what makes the check
/// discriminating: a client that grew whichever pane asked would move the widths the same way both
/// times.
fn check_the_resize_key_moves_a_boundary_on_the_daemon(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the resize key", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the resize key", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the resize key", false);
        return;
    };
    // The CURRENT WINDOW's tiled panes and their widths, which is the only set this verb can move.
    // The pane LIST spans every window of the session, so a check that measured it would be
    // reading panes no boundary here touches — and an earlier version of this check did, which is
    // how its own baseline came from a moment before its split had reflowed anything.
    let widths = |daemon: &mut HostConn| {
        let leaves = tiled_leaves(daemon, &session);
        let dims = pane_dims(daemon, &session);
        leaves
            .iter()
            .filter_map(|id| dims.get(id).map(|(cols, _)| *cols))
            .collect::<Vec<u64>>()
    };
    let before = widths(&mut daemon);

    // A boundary needs two panes IN THIS WINDOW. `prefix %` is the shipped default, and the split
    // selects the pane it opens.
    let split = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "%", false).is_ok();
    report.check("the GUI accepts `prefix %` to make a boundary", split);
    let grew = smoke.wait_for(|s| {
        let _ = s;
        let now = widths(&mut daemon);
        (now.len() > before.len()).then_some(now)
    });
    report.check(
        &format!("the split reached this window on the daemon ({before:?} -> {grew:?})"),
        grew.is_ok(),
    );
    if grew.is_err() {
        return;
    }
    // SETTLED, not merely present: a leaf appears in the arrangement before the reflow has given
    // every pane its share, so a baseline taken the instant the count changed can be a width no
    // pane ends up with. Two equal reads in a row is the condition, and it is what makes the
    // inverse assertion below mean anything.
    let mut last: Option<Vec<u64>> = None;
    let opened = smoke.wait_for(|s| {
        let _ = s;
        let now = widths(&mut daemon);
        let settled = last.as_ref() == Some(&now);
        last = Some(now.clone());
        settled.then_some(now)
    });
    report.check(
        &format!("the split's widths settled before the boundary is moved ({opened:?})"),
        opened.is_ok(),
    );
    let Ok(opened) = opened else { return };

    // `prefix C-Left` moves a boundary LEFT. WHICH boundary depends on where the split left the
    // active pane, so the claim is the one that holds for every arrangement: a width moved, and the
    // opposite key puts it back.
    //
    // It deliberately does NOT assert that one pane gave up what another took: stacked panes share
    // their columns, so a sum over widths is not a conserved quantity. That claim is made where the
    // arrangement is known, in the daemon's own tests and the CLI's.
    let pressed =
        smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "ArrowLeft", true).is_ok();
    report.check("the GUI accepts `prefix C-Left`", pressed);
    let moved = smoke.wait_for(|s| {
        let _ = s;
        let now = widths(&mut daemon);
        (now.len() == opened.len() && now != opened).then_some(now)
    });
    report.check(
        &format!("`prefix C-Left` moved a boundary on the daemon ({opened:?} -> {moved:?})"),
        moved.is_ok(),
    );
    let Ok(moved) = moved else { return };

    // WHICH leaf the boundary sits beside, learned rather than assumed — the arrangement here is
    // whatever the checks before this one left, so a fixed index would be a fixture claim.
    let Some(edge) = (0..opened.len()).find(|i| opened.get(*i) != moved.get(*i)) else {
        report.check("a leaf beside the moved boundary can be named", false);
        return;
    };
    let shrank = moved[edge] < opened[edge];
    report.check(
        &format!("`prefix C-Left` took cells from the leaf on the boundary's left (leaf {edge})"),
        shrank,
    );

    // A key bound to NOTHING, to close whatever is open. This is not padding: `resize-pane` is
    // `-r`, so the prefix table is still armed and a `prefix` typed now would be the SELF-SEND —
    // the key would go to the shell and the arrow after it would follow. An unbound key is
    // swallowed and ends the window whether one was open or not, which is the only disarm that is
    // correct in both states. (The pty test found this first, one surface over.)
    let _ = smoke.press(pane, "k", false);

    // THE DISCRIMINATOR: the opposite flag from the same pane moves the SAME boundary the OTHER
    // way. A client that resized on any arrow, or one that grew whichever pane asked, would move
    // that leaf the same way twice.
    //
    // It is a SIGN and not an exact inverse, and the reason was measured rather than assumed: the
    // headless window is still settling while this check runs (it lost two columns between two of
    // these presses in the run that found it), and every pane's width moves when the window does.
    // The direction the boundary travelled is the claim that survives that; the exact arithmetic is
    // asserted where the window is fixed, in the daemon's own tests and the CLI's.
    let pressed =
        smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "ArrowRight", true).is_ok();
    report.check("the GUI accepts `prefix C-Right`", pressed);
    let back = smoke.wait_for(|s| {
        let _ = s;
        let now = widths(&mut daemon);
        (now.len() == moved.len() && now.get(edge) > moved.get(edge)).then_some(now)
    });
    report.check(
        &format!("`prefix C-Right` moved the same boundary the other way ({moved:?} -> {back:?})"),
        back.is_ok(),
    );
}

/// The pane ids the CURRENT window TILES, in the arrangement's own order.
///
/// Read from the layout slot rather than the pane list because those answer different questions:
/// `panes` says WHO the session holds, across every window and including floating ones, and
/// `layout` says WHERE — which is the only set a boundary can move.
fn tiled_leaves(daemon: &mut HostConn, session: &str) -> Vec<u64> {
    daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/layout", "session": session }),
        )
        .ok()
        .and_then(|layout| layout["tree"]["nodes"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|node| node["leaf"].as_u64())
        .collect()
}

/// **`prefix x` asks before it kills, the question names the escalation, and a yes takes the pane
/// off the DAEMON** — R309, and the GUI half of the guarded key.
///
/// This closes a gap the debt register carried from R306: the pty test drives `prefix &` and answers
/// both ways, so `sprag-tui`'s guarded arm is live — but `sprag-gui`'s `confirm::arm_bound` was
/// reached by unit tests alone, and the two frontends' `perform` is each client's own code. The
/// register said it was "one check away"; this is that check.
///
/// What only a live GUI can say, and a unit test cannot:
///
/// * the shipped binary turns `prefix x` into an ARMED confirmation with a sentence in it,
/// * the CONSEQUENCE line reaches the surface an operator reads, carrying the escalation this
///   client computed from the live arrangement rather than from a string in a config,
/// * and answering it reaches the daemon, judged by the DAEMON's pane list.
///
/// It leaves the pane set one smaller than it found it and says so, which is item 15's hazard
/// stated by the check that creates it.
fn check_the_guarded_kill_key_asks_and_a_yes_reaches_the_daemon(
    smoke: &mut Smoke,
    report: &mut Report,
) {
    let Ok(before) = smoke.pane_count() else {
        report.check("the smoke can count panes for the guarded kill", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the guarded kill", false);
        return;
    };
    // PAST THE REPEAT WINDOW, on the hazard R308 measured: a check placed after a `-r` one inherits
    // its armed prefix table, inside which `C-b` is `send-prefix` and the next key goes to the pane.
    // Nothing enforces this, so every check that presses a prefix states it.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "x", false).is_ok();
    report.check("the GUI accepts `prefix x`", pressed);

    let prompt = smoke
        .wait_for(|s| {
            let text = s.query("sprag_confirm", "prompt").ok()?;
            text.as_str().map(str::to_owned)
        })
        .ok();
    report.check(
        &format!("`prefix x` armed a question rather than killing (prompt: {prompt:?})"),
        prompt.as_deref().is_some_and(|p| p.contains("Kill pane")),
    );
    // NOTHING DESTROYED BY THE ASKING — the half that discriminates a client which kills first and
    // asks afterwards, which would satisfy every assertion below.
    report.check(
        "and nothing is destroyed by the asking",
        smoke.pane_count().is_ok_and(|count| count == before),
    );
    if prompt.is_none() {
        return;
    }

    // THE CONSEQUENCE LINE. With more than one pane in this window there is none, because nothing
    // else is about to go — read as the CONTROL that this client is computing the escalation rather
    // than always printing it. (The escalation's positive half is driven on a real pty in
    // `sprag-tui`'s `the_pane_kill_key_says_what_it_will_take_and_takes_it`, where the harness can
    // arrange a window down to its last pane without ending the run's own client.)
    let consequence = smoke.query("sprag_confirm", "consequence").ok();
    report.check(
        &format!(
            "a pane with siblings takes nothing else, and the question says so ({consequence:?})"
        ),
        consequence.is_some_and(|value| value.as_str().is_none()),
    );

    report.check(
        "the armed kill is answerable over RPC",
        smoke.invoke("sprag_confirm", "accept", Value::Null).is_ok(),
    );
    let shrunk = smoke.wait_for(|s| {
        let count = s.pane_count().ok()?;
        (count + 1 == before).then_some(count)
    });
    report.check(
        &format!("a yes reached the daemon and the pane is gone ({shrunk:?})"),
        shrunk.is_ok(),
    );
}

/// **`prefix ?` opens this client's key table, and what it shows is the table in force.**
///
/// The GUI half of R308, driven through the shipped binary. The rows and their order are the shared
/// module's and are unit-tested there; what only this can say is that `sprag-gui` opens the panel at
/// all, that the panel holds the keyboard, and that a key gets it back — three things that live in
/// this binary's own modal code and in nothing a unit test of `sprag-host` touches. It is the same
/// argument R305 made when it added a smoke check for the window keys rather than inferring the GUI
/// from `sprag-tui`'s pty test.
///
/// The rows are read off the PAINTED TREE and not off the panel's accessible value, though the panel
/// publishes one: the claim under test is that a shipped binary put these words on a screen, and the
/// painted strings are the only thing that says so. An accessible value could be right about a panel
/// that laid out nothing.
fn check_the_key_table_opens_and_shows_the_table_in_force(smoke: &mut Smoke, report: &mut Report) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the key table", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the key table", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the key table", false);
        return;
    };
    // PAST THE REPEAT WINDOW FIRST, and this is not padding. The check before this one drives
    // `resize-pane`, which is `-r`: while its window is open the prefix table is still armed, so
    // `C-b` means `send-prefix` (the self-send) rather than arming anything, and the `?` after it
    // would go to the pane. The first run of this check passed and the second timed out on exactly
    // that — a REAL defect in the check, not a flake, and the same hazard `sprag-tui`'s own resize
    // test states one crate over. It is item 15's "a check leaves a state for whatever follows it",
    // on a resource nothing had noticed a check could leave behind.
    std::thread::sleep(sprag_host::keymap::DEFAULT_REPEAT_TIME + POLL);
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, "?", false).is_ok();
    report.check("the GUI accepts `prefix ?`", pressed);
    let opened = smoke.wait_for_tag("sprag_keyhelp_panel");
    report.check(
        &format!("`prefix ?` opened the key table ({opened:?})"),
        opened.is_ok(),
    );
    if opened.is_err() {
        return;
    }

    // WHAT IT SAYS, read off the strings the panel actually PAINTED. Not off its accessible value
    // and not off the keymap this process could build for itself: the claim is that a shipped binary
    // put these words on a screen, and only the painted tree can say that.
    let text = opened
        .as_ref()
        .ok()
        .and_then(|tags| tags.get("sprag_keyhelp_panel"))
        .map(|painted| painted.text.join("\u{1f}"))
        .unwrap_or_default();
    report.check(
        &format!("the panel painted its rows ({} strings)", text.len()),
        !text.is_empty(),
    );
    // A CHORD and its ACTION, spelled as this user presses it. `C-b z` and not `prefix z`: a view
    // that printed the word would be one a reader has to look the prefix up for, which is the
    // failure R235 met and `sprag list-keys` has avoided since.
    report.check(
        "...and it names a chord the way a user presses it",
        text.contains("C-b z"),
    );
    report.check(
        "...beside the action that chord runs",
        text.contains("zoom-pane"),
    );
    // IT SCROLLS, and the thing past the fold is THE SECOND QUESTION — what else could be bound —
    // which is the half the rival's own help cannot answer at all, because its vocabulary is a
    // struct nobody enumerates. Asserted absent first: a panel that showed everything at once would
    // make the paging below prove nothing.
    let form = "split-window -h|-v";
    report.check(
        "the last section starts off screen, so paging is a real gesture",
        !text.contains(form),
    );
    for _ in 0..4 {
        let _ = smoke.press(pane, "PageDown", false);
    }
    let paged = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        let painted = tags.get("sprag_keyhelp_panel")?.text.join("\u{1f}");
        painted.contains(form).then_some(())
    });
    report.check(
        &format!("...and paging reaches the forms a binding can name ({paged:?})"),
        paged.is_ok(),
    );

    // THE KEYBOARD IS THE PANEL'S while it is up: `prefix %` would split the window on any other
    // frame, and here it must do nothing at all.
    //
    // ⚠ WHAT THIS ONE DISCRIMINATES, stated because the revert-proof measured it: deleting the
    // `keyhelp::is_open()` gate from `route_key` leaves THIS assertion passing, because the modal's
    // focus trap holds the focus on the panel and `route_key` drops a keystroke whose focus is not a
    // pane. Two mechanisms keep the key away and this reads their conjunction. The gate itself is
    // pinned by the paging above and the close below, both of which the same edit fails.
    // Judged on the DAEMON's pane list, never on this client's paint: a client that split and
    // painted nothing would satisfy a check made on its own tree.
    let panes_before = daemon_panes(&mut daemon, &session);
    let _ = smoke.press(pane, "b", true);
    let _ = smoke.press(pane, "%", false);
    let panes_after = daemon_panes(&mut daemon, &session);
    report.check(
        &format!("no key reaches the panes behind it ({panes_before:?} -> {panes_after:?})"),
        panes_before == panes_after,
    );

    // AND A KEY GIVES THEM BACK.
    let closed = smoke.press(pane, "Escape", false).is_ok();
    report.check("the GUI accepts the `Escape` that closes it", closed);
    let gone = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_keyhelp_panel")).then_some(())
    });
    report.check(
        &format!("...and the panel is gone, so the panes have the keyboard ({gone:?})"),
        gone.is_ok(),
    );
}

/// **`prefix ,` opens this client's name prompt, and what is typed into it renames the window ON
/// THE DAEMON.**
///
/// The check that closes two things at once. The keymap ARM is shared between the frontends and the
/// prompt's POLICY is shared, but the surface that paints a modal and forwards a key into a pinion
/// field is this binary's own code — so a round that drove only `sprag-tui`'s pty test would be
/// inferring this client from a file it does not use (R305's finding, applied on the round that
/// recorded it). And it is the first live driver of a RENAME in `sprag-gui` at all, which is what
/// item 29 was left holding.
///
/// Judged against the DAEMON's window list, never against this client's paint: a client that
/// renamed nothing and painted the new name anyway would satisfy any check made on its own tree.
fn check_the_rename_key_asks_and_the_answer_reaches_the_daemon(
    smoke: &mut Smoke,
    report: &mut Report,
) {
    let Some(session) = smoke.attached_session() else {
        report.check("the window names its session for the rename key", false);
        return;
    };
    let Ok(mut daemon) = smoke.daemon() else {
        report.check("the smoke reaches the daemon for the rename key", false);
        return;
    };
    // ANY pane, read back rather than assumed to be slot 0 — the slots are not
    // re-packed, so an earlier check closing a pane frees the one this used to name.
    let Some(pane) = smoke.focus_first_pane() else {
        report.check("a pane can be focused to drive the rename key", false);
        return;
    };
    let before = windows_of(&mut daemon, &session);
    let Some(current) = before
        .iter()
        .find(|(_, current)| *current)
        .map(|(name, _)| name.clone())
    else {
        report.check("the session has a current window to rename", false);
        return;
    };

    // `prefix ,` — tmux's rename key, which before this round was `Routed::Swallow`.
    let pressed = smoke.press(pane, "b", true).is_ok() && smoke.press(pane, ",", false).is_ok();
    report.check("the GUI accepts `prefix ,`", pressed);
    let asked = smoke.wait_for_tag("sprag_prompt_panel");
    report.check(
        &format!("`prefix ,` opened the name prompt ({asked:?})"),
        asked.is_ok(),
    );
    if asked.is_err() {
        return;
    }

    // The field is focused HERE rather than left to the modal's own trap, for the reason
    // [`Smoke::focus_pane`] already records: a `focus_request` needs a winit input tick to drain and
    // there is none headless, so any focus sprag ASKS for never arrives.
    let focused = smoke.focus_tag("sprag_prompt_field");
    report.check("the prompt's field can hold the keyboard", focused);

    // The CHARACTER goes through the field's own External, and that is the recorded rule rather
    // than a shortcut: a headless run cannot make pinion's field accept a synthesised keystroke, so
    // the fix is to drive the surface by intent instead of pressing harder (the palette's `open`
    // verb exists for the same reason). What that leaves un-driven is one hop of pinion's, and
    // every hop of SPRAG's is still a key — `prefix ,` above opened this prompt, and the `Enter`
    // below is routed through `route_key` into `prompt::handle_key`, which is the code that reads
    // the field, checks the grammar and calls the daemon.
    let typed = smoke.invoke("sprag_prompt_field", "key", json!("z")) == Ok(Value::Bool(true));
    report.check("the prompt's field takes a character", typed);
    let held = smoke.query("sprag_prompt_field", "text");
    report.check(
        &format!("...and HOLDS it, seed and all ({held:?})"),
        held == Ok(Value::String(format!("{current}z"))),
    );
    let answered = smoke.press(pane, "Enter", false).is_ok();
    report.check("the GUI accepts the `Enter` that answers", answered);
    let wanted = format!("{current}z");
    let renamed = smoke.wait_for(|s| {
        let _ = s;
        let now = windows_of(&mut daemon, &session);
        now.iter()
            .any(|(name, current)| *current && *name == wanted)
            .then_some(now)
    });
    report.check(
        &format!("the typed name reached the DAEMON as {wanted:?} ({renamed:?})"),
        renamed.is_ok(),
    );
    // The AMENDED name is what proves the seed was real: the field opened holding the window's
    // current name with the caret at its end, so one keystroke appends rather than replaces. A
    // prompt that opened empty would have renamed the window to `z`.
    report.check(
        "...which is the window's OWN name with the keystroke appended, so the seed was real",
        renamed.is_ok() && wanted != "z",
    );
    let closed = smoke.wait_for(|s| {
        let tags = s.tags().ok()?;
        (!tags.contains_key("sprag_prompt_panel")).then_some(())
    });
    report.check(
        "and answering closed the prompt, giving the keyboard back",
        closed.is_ok(),
    );
}

/// Every window of `session` and which one is current, straight off the daemon — the authority the
/// window keys are judged against, since a client that painted a window the daemon does not hold
/// would satisfy any check made on its own tree.
fn windows_of(daemon: &mut HostConn, session: &str) -> Vec<(String, bool)> {
    daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/windows", "session": session }),
        )
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| {
            Some((
                w["name"].as_str()?.to_owned(),
                w["current"].as_bool().unwrap_or(false),
            ))
        })
        .collect()
}

/// Every session the DAEMON lists, in its own order — the same rows `sprag ls` and the GUI's
/// session rail paint, since all three come off one builder.
fn sessions_of(daemon: &mut HostConn) -> Vec<String> {
    daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/sessions" }),
        )
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Some(s["name"].as_str()?.to_owned()))
        .collect()
}

/// How many clients the daemon counts as VIEWING `session` — the fact a `switch-client` moves, and
/// the only one that says where a client actually is. A screen cannot establish it.
fn attached_to(daemon: &mut HostConn, session: &str) -> u64 {
    daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/sessions" }),
        )
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .find(|s| s["name"].as_str() == Some(session))
        .and_then(|s| s["attached"].as_u64())
        .unwrap_or(0)
}

/// The session's arbitrated window, straight off the daemon — the derived answer every client is
/// meant to agree with, and the one number a policy change is visible in.
fn window_size(daemon: &mut HostConn, session: &str) -> Option<(u16, u16)> {
    let value = daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/window_size", "session": session }),
        )
        .ok()?;
    Some((
        u16::try_from(value["cols"].as_u64()?).ok()?,
        u16::try_from(value["rows"].as_u64()?).ok()?,
    ))
}

/// Every attached client's reported area, in the daemon's own client list.
fn client_sizes(daemon: &mut HostConn, session: &str) -> Vec<Option<(u16, u16)>> {
    daemon
        .call(
            "scene/query",
            json!({ "path": "/sprag_mux/external/clients", "session": session }),
        )
        .ok()
        .into_iter()
        .flat_map(|list| list.as_array().cloned().unwrap_or_default())
        .filter(|client| client["session"].as_str() == Some(session))
        .map(|client| {
            Some((
                u16::try_from(client["size"]["cols"].as_u64()?).ok()?,
                u16::try_from(client["size"]["rows"].as_u64()?).ok()?,
            ))
        })
        .collect()
}

/// Each pane's `(cols, rows)`, by pane id, off the host's own pane list.
fn pane_dims(daemon: &mut HostConn, session: &str) -> std::collections::HashMap<u64, (u64, u64)> {
    let mut dims = std::collections::HashMap::new();
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
                dims.insert(id, (cols, rows));
            }
        }
    }
    dims
}

/// Each pane's `(cols, rows)` once they have stopped moving — a size read mid-reflow describes a
/// pane that will not exist by the time anything is measured against it.
fn settled_pane_dims(
    smoke: &mut Smoke,
    daemon: &mut HostConn,
    session: &str,
) -> std::collections::HashMap<u64, (u64, u64)> {
    let mut previous = None;
    smoke
        .wait_for(|_| {
            let now = pane_dims(daemon, session);
            let still = !now.is_empty() && previous.as_ref() == Some(&now);
            previous = Some(now.clone());
            still.then_some(now)
        })
        .unwrap_or_default()
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
/// Whether the DAEMON's own view of pane `pane` contains `needle` — the read that tells a client's
/// silence apart from a shell's.
///
/// Asked TWICE, and the pair is the point. `full_text` includes SCROLLBACK, so it answers "did the
/// program ever say this"; the live cell frame answers "can anything looking at this pane still see
/// it". A line in the first and not the second has scrolled out of a small pane, and no client can
/// be blamed for not painting it — that is a check hunting a transient string, not a product that
/// lost one.
///
/// `None` from either read when the daemon would not answer at all, which is a third thing again
/// and must not read as "no".
fn pane_holds(
    daemon: &mut HostConn,
    session: &str,
    pane: u64,
    needle: &str,
) -> (Option<bool>, Option<bool>) {
    let mut ask = |slot: String| {
        daemon
            .call(
                "scene/query",
                json!({
                    "path": format!("/pane_{pane}/sprag_input/external/{slot}"),
                    "session": session,
                }),
            )
            .ok()
            .map(|value| value.to_string().contains(needle))
    };
    (ask("full_text".to_owned()), ask("cells.0".to_owned()))
}

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
    /// The logs of the clients this run has ALREADY replaced — see [`Smoke::gui_log`].
    prior_gui_logs: Vec<PathBuf>,
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
        install_notify_stand_in(&state)?;

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
            prior_gui_logs: Vec::new(),
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

    /// Every tagged node in the main window's PAINTED tree, or WHY the tree could not be read.
    ///
    /// `from: "paint"` is the displayed frame — real pixels; `"state"` is the pre-paint tree and
    /// would let a check pass on geometry that was never shown. The path is `/window[main]` with an
    /// EMPTY scene tail: a snapshot is a whole-tree dump, so a bare tag (or even `"/"`) is refused.
    ///
    /// # The `Result` is the point, and it is here because collapsing it cost a green tick
    ///
    /// This read used to swallow a failed `scene/snapshot` into an EMPTY map, which made "the client
    /// answered, and paints nothing" and "the client did not answer" the same value everywhere
    /// downstream — and the two are opposite diagnoses. It produced both failure modes a swallowed
    /// error can produce, one round apart:
    ///
    /// * a FALSE FAILURE — [`check_terminal_output_never_reaches_the_shaper`] reported `painted []`
    ///   as a pane-correspondence disagreement, roughly one run in ten;
    /// * a FALSE PASS, which is worse — "the palette starts unpainted" is `!contains_key`, so an
    ///   unanswered snapshot would have PASSED it while proving nothing at all.
    ///
    /// So every reader below propagates rather than defaults, and each caller states which meaning
    /// it wants: inside a [`Smoke::wait_for`] closure an unreadable tree is honestly "not yet"
    /// (`.ok()?`), and in a one-shot assertion it is a failure that names itself.
    fn tags(&mut self) -> Result<std::collections::HashMap<String, Painted>, String> {
        let value = self.call(
            "scene/snapshot",
            json!({ "path": "/window[main]", "from": "paint" }),
        )?;
        let mut out = std::collections::HashMap::new();
        walk(value.get("scene").unwrap_or(&value), &mut out);
        Ok(out)
    }

    /// What a painted `TextGrid` shows and what it holds: `(painted, buffer)`, each `(cols, rows)`.
    ///
    /// The two are the whole of the crop question. A grid whose painted size is smaller than its
    /// buffer is showing PART of its pane — which is either the honest consequence of a policy the
    /// user chose, or the silent truncation this round's gate exists to catch, and the numbers are
    /// the same either way. Only the snapshot can answer it: the client's own scene carries both,
    /// and no daemon query knows what a window drew.
    fn grid_facts(&mut self) -> Vec<GridFacts> {
        let Ok(value) = self.call(
            "scene/snapshot",
            json!({ "path": "/window[main]", "from": "paint" }),
        ) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect_grids(value.get("scene").unwrap_or(&value), &mut out);
        out
    }

    /// Attach a real `sprag-tui` to `session` on a real pseudoterminal of `cols` x `rows`.
    ///
    /// A PTY rather than a pipe, and the binary rather than a stand-in, because everything this
    /// gates is downstream of a terminal client being one: it reads its window size from the tty,
    /// reports THAT to the daemon, and a client whose size came from anywhere else would prove
    /// nothing about the arbitration. Spawned through sprag's own [`PanePty`](sprag_terminal::PanePty), so the harness runs
    /// the same PTY authority a pane does.
    ///
    /// The environment is spelled out rather than inherited: the client must reach THIS run's
    /// daemon and read THIS run's config, and a stray `SPRAG_*` in the developer's shell reaching a
    /// child here is the class of leak an isolated run exists to prevent.
    ///
    /// Dropping the returned handle kills the client, which is how the detach half is driven.
    fn attach_terminal(
        &self,
        session: &str,
        cols: u16,
        rows: u16,
    ) -> Result<sprag_terminal::PanePty, String> {
        let mut command = sprag_terminal::CommandBuilder::new(self.target.join("sprag-tui"));
        command.env("SPRAG_GUI_SESSION", session);
        command.env("SPRAG_GUI_HOST_SOCK", &self.host_sock);
        command.env("SPRAG_HOST_RPC_SOCK", &self.host_sock);
        command.env("XDG_STATE_HOME", &self.state);
        command.env("XDG_CONFIG_HOME", self.state.join("config"));
        command.env("TERM", "xterm-256color");
        sprag_terminal::PanePty::spawn(command, cols, rows)
            .map_err(|error| format!("could not attach a terminal client: {error}"))
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
    fn docked_panes(&mut self) -> Result<Vec<usize>, String> {
        let mut indices: Vec<usize> = self
            .tags()?
            .keys()
            .filter_map(|tag| tag.strip_prefix("sprag_gui.pane.")?.parse().ok())
            .collect();
        indices.sort_unstable();
        Ok(indices)
    }

    /// How many pane tiles the main window is painting.
    fn pane_count(&mut self) -> Result<usize, String> {
        Ok(self.docked_panes()?.len())
    }

    /// Write the user's `config.toml` — the file both children read out of this run's isolated
    /// `XDG_CONFIG_HOME`, and the one the client's keymap comes from.
    ///
    /// Writable mid-run because the file IS the live table: `sprag bind-key` edits it and a running
    /// client is supposed to act on that, so a check has to be able to do the same.
    fn write_user_config(&mut self, text: &str) -> Result<(), String> {
        let dir = self.state.join("config").join("sprag");
        std::fs::create_dir_all(&dir).map_err(|error| format!("config dir: {error}"))?;
        std::fs::write(dir.join("config.toml"), text).map_err(|error| format!("config: {error}"))
    }

    /// Hold `mods` down, press `key`, and let go — one keystroke through the shell's own keyboard
    /// gate, landing in `apply_key` exactly as a physical one does.
    ///
    /// The modifiers are a SEPARATE call because `scene/key` carries none: the substrate holds a
    /// modifier state that `scene/modifiers` writes, and `apply_key` is handed whatever is held at the
    /// moment the key dispatches. Released afterwards so a chord cannot leak into the next press.
    ///
    /// `path` rather than a pixel coordinate — the key needs a position only because the drain moves
    /// the cursor first, and naming the pane's TAG keeps this readable when the layout changes.
    fn press(&mut self, pane: usize, key: &str, ctrl: bool) -> Result<(), String> {
        if ctrl {
            self.call("scene/modifiers", json!({ "ctrl": true }))?;
        }
        let path = format!("sprag_gui.pane.{pane}");
        let sent = self.call("scene/key", json!({ "path": path, "key": key }));
        if ctrl {
            self.call("scene/modifiers", json!({}))?;
        }
        sent.map(|_| ())
    }

    /// Put the within-app focus on a pane that ACTUALLY EXISTS, answering which slot it landed on.
    ///
    /// The honest form of [`focus_pane`](Self::focus_pane) for the twelve checks that mean *any
    /// pane*: pane SLOTS are not re-packed as panes come and go, so slot 0 is free the moment a
    /// check earlier in the run closes the pane that had it — and a key check that then pressed at
    /// `sprag_gui.pane.0` would be typing into nothing.
    ///
    /// **Measured, not anticipated** (R331): `a pane can be focused to drive the order keys` failed
    /// one run in three at exactly that, after `prefix n` walked the client onto another window, and
    /// R331's own new check failed it every time. A flake is a bug rather than something to retry,
    /// and the bug was in the harness's assumption.
    ///
    /// The FIRST docked pane rather than a chosen one, because these callers do not care which:
    /// what they need is a pane the keyboard can be pointed at. A check that needs a SPECIFIC pane
    /// still says so with [`focus_pane`](Self::focus_pane).
    fn focus_first_pane(&mut self) -> Option<usize> {
        let pane = self.docked_panes().ok()?.first().copied()?;
        self.focus_pane(pane).then_some(pane)
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
        self.focus_tag(&format!("sprag_gui.pane.{i}"))
    }

    /// Put the within-app focus on `tag`, reading it back — [`focus_pane`](Self::focus_pane)
    /// generalised, because a MODAL's field needs the same accommodation for the same reason: the
    /// focus a modal REQUESTS when it opens rides on a `focus_request`, and nothing drains one
    /// headlessly.
    fn focus_tag(&mut self, tag: &str) -> bool {
        let _ = self.call("focus/set", json!({ "tag": tag }));
        self.focused().as_deref() == Some(tag)
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
    fn tabs(&mut self) -> Result<Vec<String>, String> {
        let painted = self.tags()?;
        Ok((0..)
            .map_while(|i| painted.get(&format!("sprag_gui.wtab.{i}")))
            .filter_map(|node| node.text.first().cloned())
            .collect())
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
            let tags = smoke.tags().ok()?;
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

    /// Everything EVERY GUI this run started has written to stderr. Missing / unreadable reads as
    /// empty, which the caller must treat as "no evidence", never as "no problem".
    ///
    /// Every log, not the newest: [`relaunch_gui`](Self::relaunch_gui) starts a second client, and a
    /// reader that saw only its log would narrow a claim about "every painted frame" to the last
    /// client's frames while still reading like a claim about the run.
    fn gui_log(&self) -> String {
        self.prior_gui_logs
            .iter()
            .chain(std::iter::once(&self.gui_log))
            .map(|path| std::fs::read_to_string(path).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What pane tile `i` is SHOWING, one string per painted row.
    ///
    /// An ABSENT tile answers an empty vec — that tile is genuinely painting nothing here, which is
    /// what a floated pane looks like — while an unreadable TREE is an error, because then nothing
    /// is known about any tile.
    fn pane_rows(&mut self, i: usize) -> Result<Vec<String>, String> {
        Ok(self
            .tags()?
            .remove(&format!("sprag_gui.pane.{i}"))
            .map(|pane| pane.rows)
            .unwrap_or_default())
    }

    /// How many frames the client has presented.
    fn frame_count(&mut self) -> u64 {
        self.call("scene/frame_timings", json!({}))
            .ok()
            .and_then(|timings| timings["frame_count"].as_u64())
            .unwrap_or_default()
    }

    /// Kill this run's GUI and start a fresh one against the same daemon, reconnecting to its scene.
    ///
    /// **The only way to gate a setting a window adopts at BIRTH.** `gui-font` is one: the glyph size
    /// decides the measured cell, which decides every pane's grid, so changing it live is the resize
    /// path plus a wake the client deliberately does not have. A check that merely wrote the file
    /// under a running window would therefore be asserting nothing — and would pass just as well
    /// against a client that never read the option at all.
    ///
    /// The stale socket file is removed first. `serve` reclaims a path nobody answers on, so the new
    /// client would come up anyway; removing it is what makes [`wait_for_path`] mean "the new one is
    /// listening" rather than "the dead one's file is still there".
    ///
    /// Its log goes to a SECOND file, so a failure after this can still be read against the first
    /// launch's output — `spawn` creates the log, and reusing the path would truncate it.
    fn relaunch_gui(&mut self) -> Result<(), String> {
        let _ = self.gui.kill();
        let _ = self.gui.wait();
        let _ = std::fs::remove_file(&self.gui_sock);
        self.prior_gui_logs.push(self.gui_log.clone());
        self.gui_log = self
            .state
            .join(format!("gui-relaunch-{}.log", self.prior_gui_logs.len()));
        self.gui = spawn(
            &self.target.join("sprag-gui"),
            &self.host_sock,
            &self.gui_sock,
            &self.state,
            &self.gui_log,
        )
        .map_err(|error| format!("relaunch the gui: {error}"))?;
        wait_for_path(&self.gui_sock).map_err(|error| error.to_string())?;
        self.conn =
            HostConn::connect(&self.gui_sock, PATIENCE).map_err(|error| error.to_string())?;
        // The OS-focus gate `boot` applies to the first launch: without it `os_focused_window` is null
        // under Xvfb and anything reading the focused pane reads nothing.
        self.call("scene/window_focus", json!({ "focused": true }))
            .map_err(|error| format!("focus the relaunched window: {error}"))?;
        Ok(())
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
        // The stand-in `notify-send` FIRST on the path — see [`install_notify_stand_in`]. It is set
        // for the whole run rather than for one check because the claim under test is partly a
        // NEGATIVE one ("nothing reached the desktop while the person was here"), and a recorder
        // installed only around the positive half could not see the messages that came before it.
        .env("PATH", stand_in_path(state))
        .env("SPRAG_HOST_RPC_SOCK", host)
        .env("SPRAG_GUI_HOST_SOCK", host)
        .env("SPRAG_RPC_SOCK", gui)
        .env("XDG_STATE_HOME", state)
        // The user's CONFIG, isolated the same way the state is. Both children read it — the daemon
        // for the palette's declared commands, the client for its keymap — so without this a run is
        // reading whatever `config.toml` the developer happens to have, and a `[[command]]` in it
        // would silently change a palette row count this file asserts on.
        .env("XDG_CONFIG_HOME", state.join("config"))
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

/// Where the stand-in `notify-send` is written, and what its record file is called.
///
/// The GUI's desktop notification is a PROGRAM it runs, so the honest way to read what it sent is
/// to be that program. A recorder on `PATH` sees the real argv the real binary built — no seam in
/// the product, no injection point that a shipped path could route around.
fn notify_stand_in_dir(state: &Path) -> PathBuf {
    state.join("bin")
}

/// The file the stand-in appends one line per invocation to.
fn notify_record(state: &Path) -> PathBuf {
    state.join("notify-send.log")
}

/// `PATH` with the stand-in's directory in FRONT of the inherited one.
fn stand_in_path(state: &Path) -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!("{}:{inherited}", notify_stand_in_dir(state).display())
}

/// Write the stand-in `notify-send` and make it executable.
///
/// Each line is `<caller pid>` then the argv, with a unit separator between fields so an argument
/// containing a space cannot be read as two. It exits 0, because the product must not be tested
/// against a notifier that is failing.
///
/// **The pid is what makes a reading an ATTRIBUTION.** Both children inherit this `PATH`, so a
/// recorder that logged only the argv would answer *somebody ran the notifier* — and the claim under
/// test is that the CLIENT does, which a daemon doing it would satisfy while being the wrong design.
/// `$PPID` inside the script is the process that spawned it, which is the client itself: its
/// notifier thread is a thread of that process, not a helper of its own.
fn install_notify_stand_in(state: &Path) -> io::Result<()> {
    let dir = notify_stand_in_dir(state);
    std::fs::create_dir_all(&dir)?;
    let record = notify_record(state);
    let script = format!(
        "#!/bin/sh\nprintf '%s\\037' \"$PPID\" \"$@\" >> {}\nprintf '\\n' >> {}\nexit 0\n",
        record.display(),
        record.display(),
    );
    let path = dir.join("notify-send");
    std::fs::write(&path, script)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
}

/// Every notifier invocation the stand-in has recorded, as `(caller pid, arguments)`.
fn notify_records(state: &Path) -> Vec<(u32, Vec<String>)> {
    std::fs::read_to_string(notify_record(state))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}').filter(|field| !field.is_empty());
            let pid = fields.next()?.parse().ok()?;
            Some((pid, fields.map(str::to_owned).collect()))
        })
        .collect()
}

/// The invocations made by `pid` — the CLIENT's, so a daemon that started notifying could never be
/// read as the client doing its job.
fn notify_calls_by(state: &Path, pid: u32) -> Vec<Vec<String>> {
    notify_records(state)
        .into_iter()
        .filter(|(caller, _)| *caller == pid)
        .map(|(_, argv)| argv)
        .collect()
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

/// What one painted pane grid SHOWS and what it HOLDS, each `(cols, rows)`.
///
/// The pair is the whole of the crop question, which is why it travels as one value: a grid whose
/// painted size is smaller than its buffer is showing part of its pane, and either number alone
/// says nothing about that.
/// A `painted < buffer on both axes` wait that PRINTS what it last saw when it does not hold.
///
/// The predicate is the same at both call sites and a bare `false` from either is undiagnosable:
/// "the window paints part of every pane" failing tells a reader nothing about WHICH pane, whether
/// the grids were empty, or which axis was equal. Observed flaking once in seventeen runs with no
/// evidence of the cause, which is what a silent gate costs.
fn every_pane_is_cropped(smoke: &mut Smoke, what: &str) -> bool {
    let mut last = Vec::new();
    let held = smoke
        .wait_for(|smoke| {
            let grids = smoke.grid_facts();
            let cropped = !grids.is_empty()
                && grids
                    .iter()
                    .all(|g| g.widget.0 < g.buffer.0 && g.widget.1 < g.buffer.1);
            last = grids;
            cropped.then_some(())
        })
        .is_ok();
    if !held {
        eprintln!("      {what}: last grids (painted, buffer) = {last:?}");
    }
    held
}

/// The three sizes a pane grid carries, which used to be two.
///
/// Until PINION-PR80 landed, a node's `cols`/`rows` WERE the widget's span: pinion derived them
/// from the laid-out rect, and the checks above read them as "what this client painted". The node
/// now DECLARES the daemon's grid instead, so `cols == buffer_cols` by construction — which is the
/// promotion, and which would have made those checks vacuously true had they gone on reading
/// `cols`. The widget's span is not lost, it moved: it is `rect / cell metric`, the derivation the
/// node stopped answering with, and the snapshot still carries both halves of it.
#[derive(Debug)]
struct GridFacts {
    /// Whole cells the pane's WIDGET spans, from `rect` and the node-local cell metric.
    widget: (u64, u64),
    /// What the node declares the producer was sized to (`cols` / `rows`).
    declared: (u64, u64),
    /// What the producer last delivered (`buffer_cols` / `buffer_rows`).
    buffer: (u64, u64),
    /// Which authority the node names for `declared` — `"producer"` on every sprag grid.
    source: String,
}

/// Every painted pane grid in `node`'s subtree, as [`GridFacts`].
///
/// Found by SHAPE rather than by a slot number, and that is not fussiness: a slot is a display
/// position this run's earlier checks move panes in and out of, so `sprag_gui.pane.0` names an
/// empty slot after a kill and a check written to it asserts about nothing.
fn collect_grids(node: &Value, out: &mut Vec<GridFacts>) {
    let named = node["tag"]
        .as_str()
        .is_some_and(|tag| tag.starts_with("sprag_gui.pane.") && tag.ends_with("#grid"));
    if named
        && let (Some(cols), Some(rows), Some(buffer_cols), Some(buffer_rows)) = (
            node["cols"].as_u64(),
            node["rows"].as_u64(),
            node["buffer_cols"].as_u64(),
            node["buffer_rows"].as_u64(),
        )
        && let (Some((w, h)), Some(cell_w), Some(cell_h)) = (
            rect_of(&node["rect"]),
            node["cell_w"].as_u64(),
            node["cell_h"].as_u64(),
        )
        && cell_w > 0
        && cell_h > 0
    {
        out.push(GridFacts {
            widget: (u64::from(w) / cell_w, u64::from(h) / cell_h),
            declared: (cols, rows),
            buffer: (buffer_cols, buffer_rows),
            source: node["winsize_source"].as_str().unwrap_or("").to_owned(),
        });
    }
    for child in node["children"].as_array().into_iter().flatten() {
        collect_grids(child, out);
    }
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
