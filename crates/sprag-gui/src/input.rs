//! Input routing: a focused keystroke / IME commit -> the FOCUSED pane's PTY
//! through the host client (`Host::send_key` / `send_text`), the focus-cycle
//! chord that moves between tiled panes, and the per-pane scrollback-view offset
//! those keys snap.
//! The [`TerminalViewer`](crate::TerminalViewer) `apply_key` / `apply_composition`
//! trait methods delegate here. See the crate-root "Input" / "Scrollback" docs.

use crate::keys::use_client_keys;
use crate::terminal::{pane_cache_key, pane_index_of, pane_tag, use_terminal};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::{CompositionEvent, Modifiers, Scene};
use sprag_host::keymap::{BoundAction, Routed, SwitchClientAsk};
use sprag_host::wire::SelectWindowAsk;

/// `Owner::cache` key for pane `pane`'s IME preedit overlay. Minted via the one
/// per-pane key site [`pane_cache_key`] so the index suffix cannot drift.
fn preedit_key(pane: usize) -> String {
    pane_cache_key("preedit", pane)
}

/// Pane `pane`'s IME preedit (in-progress composition) string, an
/// `Owner::cache`-backed [`Signal`] that [`route_composition`] writes on each
/// composition event and `view` reads every frame to overlay at that pane's
/// cursor (see [`sprag_grid::overlay_preedit`] for why a terminal renders the
/// preedit itself — the CLIENT-side step over the host's cells). Empty = not composing;
/// display-only (the preedit never reaches the PTY — only a `Commit` writes).
/// Because `view` reads it every frame, a `set` flips the root owner dirty so the
/// shell's R705.1 reactive bridge arms a redraw — the composition repaints live
/// as you type. **Per-pane**: composition targets the focused pane only.
pub(crate) fn use_preedit(pane: usize) -> Signal<String> {
    let owner = Owner::current().expect("use_preedit() requires an active Owner scope");
    owner
        .cache(preedit_key(pane), || Signal::new(String::new()))
        .as_ref()
        .clone()
}

/// Clear pane slot `pane`'s IME preedit when the slot FREES (Round 2b live delta), so a
/// slot REUSED by a later pane shows no inherited in-progress composition. By MUTATION
/// (`set` to empty) — pinion has no `Owner::cache` evict, and `view` reads the same signal
/// each frame, so a `set` flips the root owner dirty and the cleared overlay repaints. Runs
/// in the root owner (the pre-view `reconcile_frame` hook, which resolves the cache slot).
pub(crate) fn reset_pane_preedit(pane: usize) {
    use_preedit(pane).set(String::new());
}

/// The signed row delta a `Shift+PageUp` / `Shift+PageDown` applies to the
/// top-anchored `offset_y` ([`crate::scrollbar`]: `0` = oldest top, `max` = live
/// bottom). `PageUp` walks toward older history (DECREASE `offset_y`), `PageDown`
/// back toward the live bottom (INCREASE) — `ScrollState::scroll_by` clamps to
/// `[0, max]`, so this only carries the direction. Pure / unit-testable.
fn page_delta(key: &str, page: i32) -> i32 {
    match key {
        "PageUp" => -page,
        "PageDown" => page,
        _ => 0,
    }
}

/// Scroll pane `pane`'s history for a `Shift+PageUp` / `Shift+PageDown` by one
/// page on its row-unit [`ScrollState`](crate::scrollbar::use_pane_scroll). A page
/// is the viewport height less one row (one row of overlap for continuity);
/// `scroll_by` clamps to the reconciled `[0, max]` depth. Reads the live pane for
/// the row count; called from `apply_key` (outside any cache factory).
fn scroll_view(pane: usize, key: &str) {
    let scroll = crate::scrollbar::use_pane_scroll(pane);
    let rows = use_terminal().slots.pane_scroll_facts(pane).visible_rows;
    let page = i32::from(rows).saturating_sub(1).max(1);
    scroll.scroll_by(0, page_delta(key, page));
}

/// The scroll `offset_y` to jump to for a prompt jump, or `None` for a no-op (no prompt in
/// that direction). `positions` are the OSC 133 prompt logical line indices (from the oldest
/// line, ascending — the `Screen::prompt_positions` shape);
/// `current` is the view's top line (the current `offset_y`); `max` is the scrollable bound
/// (`scrollback_len`). `ArrowUp` finds the nearest prompt ABOVE the top (largest position
/// `< current`), `ArrowDown` the nearest BELOW (smallest `> current`), clamped to `[0, max]`
/// — a prompt still in the visible grid (index `> max`) resolves to the live bottom. Pure, so
/// the jump math is unit-tested without a host.
fn jump_target(positions: &[usize], current: i32, max: i32, key: &str) -> Option<i32> {
    let current = usize::try_from(current.max(0)).unwrap_or(0);
    let max = usize::try_from(max.max(0)).unwrap_or(0);
    let target = match key {
        "ArrowUp" => *positions.iter().rev().find(|&&p| p < current)?,
        "ArrowDown" => *positions.iter().find(|&&p| p > current)?,
        _ => return None,
    };
    let clamped = target.min(max);
    // A jump that would not move the view is a no-op: the only prompt "below" is already
    // in the visible grid, so it clamps to the live bottom the view already sits on.
    if clamped == current {
        return None;
    }
    i32::try_from(clamped).ok()
}

/// Jump pane `pane`'s scroll view to the previous / next OSC 133 shell prompt
/// (`Ctrl+Shift+ArrowUp/Down`). Reads the prompt positions ON DEMAND (a keypress, never per
/// frame) and moves the row-unit [`ScrollState`](crate::scrollbar::use_pane_scroll) to the
/// target — a no-op when the shell emits no marks or there is no prompt in that direction.
fn scroll_to_prompt(pane: usize, key: &str) {
    let positions = use_terminal().slots.pane_prompt_positions(pane);
    if positions.is_empty() {
        return;
    }
    let scroll = crate::scrollbar::use_pane_scroll(pane);
    if let Some(target) = jump_target(&positions, scroll.offset_y(), scroll.max().1, key) {
        scroll.scroll_to(0, target);
    }
}

/// The slot to focus after a `Ctrl+PageUp` (previous) / `Ctrl+PageDown` (next) from
/// `active`, wrapping over the `occupied` display slots. Cycles over the OCCUPIED set
/// (skipping any hole a closed pane left — Round 2b), not a contiguous `0..count`; at
/// boot the set is contiguous so this is the former modular wrap. `None` when there is
/// nowhere to switch (`occupied.len() <= 1`, `active` not in the set, or a non-cycle
/// key). Pure, so it is unit-testable; up=previous / down=next mirrors the scrollback
/// chord.
fn next_focus(active: usize, forward: bool, occupied: &[usize]) -> Option<usize> {
    if occupied.len() <= 1 {
        return None;
    }
    let pos = occupied.iter().position(|&slot| slot == active)?;
    let n = occupied.len();
    // `n - 1` is `-1` modulo `n` — the same wrap [`session_neighbour`] uses over the session list.
    let step = if forward { 1 } else { n - 1 };
    Some(occupied[(pos + step) % n])
}

/// Move focus to the next / previous tiled pane (wrapping) via a pinion
/// [`focus_request`](pinion_core::focus_request) — the framework focus ring and
/// the `apply_key` routing both follow the focus manager, so requesting the new
/// pane's tag is the whole switch. A single-pane window is a no-op.
///
/// A `forward` flag rather than the key that asked, because two things now ask: the
/// `Ctrl+PageUp/Down` chord, which carries a direction, and the keymap's `select-pane -t :.+`,
/// which is only ever forward. Threading a key string through for the second would mean spelling a
/// chord's key at a site that has nothing to do with chords.
fn cycle_focus(active: usize, forward: bool) {
    if let Some(next) = next_focus(active, forward, &use_terminal().slots.occupied_slots()) {
        pinion_core::focus_request::request(pane_tag(next));
    }
}

/// A reserved chord that acts on the window / layout, NOT the focused pane's PTY.
/// `route_key` recognizes one via [`window_chord`] and dispatches it instead of
/// injecting.
#[derive(Debug, PartialEq, Eq)]
enum WindowChord {
    /// `Ctrl+PageUp/Down` — cycle focus between tiles (the `key` carries the
    /// direction at dispatch).
    CycleFocus,
    /// `Shift+PageUp/Down` — scroll the focused pane's history.
    Scroll,
    /// `Ctrl+Shift+Enter` — toggle the focused pane's dock state.
    ToggleDock,
    /// `Ctrl+Shift+ArrowUp/Down` — jump the focused pane's scroll view to the previous /
    /// next OSC 133 shell prompt (the `key` carries the direction at dispatch).
    JumpPrompt,
}

impl WindowChord {
    /// A stable, allocation-free name for the diagnostic trace ([`crate::diag`]).
    fn label(&self) -> &'static str {
        match self {
            Self::CycleFocus => "CycleFocus",
            Self::Scroll => "Scroll",
            Self::ToggleDock => "ToggleDock",
            Self::JumpPrompt => "JumpPrompt",
        }
    }
}

/// Whether THIS CLIENT acts on `key` + `modifiers` as one of its own reserved chords — the ones no
/// keymap holds because they are GUI capabilities (find, clipboard, the dock toggle) rather than mux
/// verbs.
///
/// The ONE place that answers it, so the palette's hint column can be checked against the same
/// predicate `route_key` uses instead of against a list somebody maintains. It exists because R314
/// shipped a hint for a chord whose recogniser had just been deleted, and nothing in the tree could
/// tell: a literal in a hint column is a claim about this file, and until now it was made nowhere.
///
/// It deliberately does NOT consult the keymap. A chord the KEYMAP holds is derived by the palette
/// through `Command::bound`, and folding the two together here would let a row satisfy the check by
/// the wrong route.
///
/// `#[cfg(test)]`, and that is honest rather than a shortcut: `route_key` cannot call it, because it
/// needs to know WHICH recogniser matched in order to act. What this adds is the OR of the three,
/// which only a checker wants — so it derives from the production functions rather than restating
/// them, and there is no fourth spelling for a new chord to be missing from.
#[cfg(test)]
pub(crate) fn client_chord_acts(key: &str, modifiers: Modifiers) -> bool {
    find_chord(key, modifiers)
        || clipboard_chord(key, modifiers).is_some()
        || window_chord(key, modifiers).is_some()
}

/// Recognize a reserved window chord from `key` + `modifiers`, or `None` for a
/// normal keystroke (which injects). Pure — the chord-decision is separated from
/// the side-effecting inject path and unit-tested directly. The page chords take
/// EXACTLY ONE of Ctrl / Shift (Ctrl-only cycles focus, Shift-only scrolls): `Ctrl+Shift+Page` is a
/// ROOT BINDING of the shared vocabulary since R314 (`switch-client -n`/`-p`), so excluding the
/// other modifier here keeps the two disjoint. It also no longer has to: the keymap route runs
/// BEFORE this function, so a bound chord never reaches it — the exclusion is kept because it is
/// what makes each chord say what it is, and because a user may unbind the session chords and
/// expect these to stay theirs. `Ctrl+Shift+Enter` is essentially unbound in TUIs (terminals cannot
/// encode it distinctly), so it steals no app key.
fn window_chord(key: &str, modifiers: Modifiers) -> Option<WindowChord> {
    let is_page = matches!(key, "PageUp" | "PageDown");
    if modifiers.ctrl && !modifiers.shift && is_page {
        Some(WindowChord::CycleFocus)
    } else if modifiers.shift && !modifiers.ctrl && is_page {
        Some(WindowChord::Scroll)
    } else if modifiers.ctrl && modifiers.shift && key == "Enter" {
        Some(WindowChord::ToggleDock)
    } else if modifiers.ctrl && modifiers.shift && matches!(key, "ArrowUp" | "ArrowDown") {
        // Ctrl+Shift+Arrow is in sprag's reserved GUI-chord space (Ctrl+Shift+Enter dock,
        // Ctrl+Shift+C/V clipboard, Ctrl+Shift+Page sessions); jump-to-prompt joins it. The
        // session Page chord takes Page, so the arrows do not shadow it.
        Some(WindowChord::JumpPrompt)
    } else {
        None
    }
}

/// A `Ctrl+Shift+C` (copy) / `Ctrl+Shift+V` (paste) clipboard chord (R139).
#[derive(Debug, PartialEq, Eq)]
enum ClipboardChord {
    Copy,
    Paste,
}

/// Recognize a clipboard chord, or `None` for a normal keystroke. Terminal convention:
/// `Ctrl+C` is SIGINT and `Ctrl+V` is literal-next, so copy / paste are the `Shift`
/// variants (matching `gnome-terminal` / xterm). `Shift` upper-cases the letter, so
/// match case-insensitively; `Alt` excluded so `Ctrl+Alt+Shift+*` is not stolen. Pure.
fn clipboard_chord(key: &str, modifiers: Modifiers) -> Option<ClipboardChord> {
    if modifiers.ctrl && modifiers.shift && !modifiers.alt {
        if key.eq_ignore_ascii_case("c") {
            return Some(ClipboardChord::Copy);
        }
        if key.eq_ignore_ascii_case("v") {
            return Some(ClipboardChord::Paste);
        }
    }
    None
}

/// Whether `key` + `modifiers` is the find-bar chord (`Ctrl+Shift+F`). Pure, and case-insensitive
/// because `Shift` upper-cases the letter; `Alt` excluded so `Ctrl+Alt+Shift+F` is not stolen —
/// the same shape [`clipboard_chord`] uses.
fn find_chord(key: &str, modifiers: Modifiers) -> bool {
    modifiers.ctrl && modifiers.shift && !modifiers.alt && key.eq_ignore_ascii_case("f")
}

/// Whether `key` + `modifiers` is the command-palette chord (`Ctrl+Shift+P`). Same shape as
/// [`find_chord`], and `P` is free in sprag's reserved `Ctrl+Shift+<letter>` space (`C`/`V`
/// clipboard, `F` find, `L` last session).
///
/// `Ctrl+Shift+P` is the palette binding VS Code, Atom and Sublime all use, so it is the first thing
/// a user tries; `Ctrl+P` is left alone because bare `Ctrl+P` is a PTY key (readline previous-line)
/// that must keep reaching the child, the same reason the find bar took the `Shift` variant.
fn palette_chord(key: &str, modifiers: Modifiers) -> bool {
    modifiers.ctrl && modifiers.shift && !modifiers.alt && key.eq_ignore_ascii_case("p")
}

/// The session to switch to when cycling from `current` by one step over `names` (the sidebar's
/// session list, in order), wrapping — `forward` to the NEXT, else the PREVIOUS. `None` when
/// `current` is not in `names` (nothing to anchor on). A single-session list yields `current` itself,
/// which `switch_session` no-ops. Pure. Shared with the sidebar keyboard cursor
/// ([`crate::stabs::handle_sidebar_key`]) so the `Ctrl+Shift+PageUp/Down` chord and the in-rail
/// `↑`/`↓` rove over the SAME wrapping list order.
pub(crate) fn session_neighbour(names: &[String], current: &str, forward: bool) -> Option<String> {
    let here = names.iter().position(|name| name == current)?;
    let len = names.len();
    let step = if forward { 1 } else { len - 1 }; // len - 1 == -1 modulo len
    Some(names[(here + step) % len].clone())
}

/// A stable, allocation-free name for a bound action in the diagnostic trace ([`crate::diag`]) —
/// the [`WindowChord::label`] shape, for the same reason: a trace line has to be readable without
/// the reader knowing which key produced it.
///
/// Spelled as the VERB the user bound rather than as the enum's variant, so a `diag` line and the
/// `config.toml` line that caused it read the same. The split's flags are left out: the label names
/// which command ran, and the arrangement it produced is in the layout the next frame paints.
fn action_label(action: &BoundAction) -> &'static str {
    match action {
        BoundAction::DetachClient => "detach-client",
        BoundAction::SendPrefix => "send-prefix",
        BoundAction::ListKeys => "list-keys",
        BoundAction::SplitWindow { .. } => "split-window",
        // Both `select-pane` forms label as the VERB: the flags say which pane, and the pane the
        // action landed on is in the next frame's layout — the same reason the split's flags are
        // left out.
        BoundAction::SelectNextPane | BoundAction::SelectPaneToward { .. } => "select-pane",
        // Its own verb, unlike the two select forms above: this one MOVES A PANE, and a label that
        // said `select-pane` would name the gesture the user did not make.
        BoundAction::SwapPaneToward { .. } => "swap-pane",
        // Its own verb for the swap's reason: this one MOVES A BOUNDARY, and the distance is left
        // out with the split's flags because the arrangement it produced is in the next frame.
        BoundAction::ResizePaneToward { .. } => "resize-pane",
        BoundAction::ZoomPane { .. } => "zoom-pane",
        BoundAction::KillPane => "kill-pane",
        BoundAction::NewWindow => "new-window",
        BoundAction::SelectWindow { .. } => "select-window",
        BoundAction::SwitchClient { .. } => "switch-client",
        BoundAction::ChooseTree => "choose-tree",
        BoundAction::KillWindow => "kill-window",
        BoundAction::RenameWindow => "rename-window",
        // Both move forms label as the VERB: the place says WHERE, and where the window landed is
        // in the next frame's window strip — the split's rule, one level up.
        BoundAction::MoveWindow { .. } | BoundAction::MoveWindowBefore => "move-window",
        BoundAction::RenameSession => "rename-session",
        BoundAction::RenamePane => "rename-pane",
        // The GUARD's own name, not the verb it guards: what this diag line records is that a key
        // opened a question, and the answer's own `perform` records what ran.
        BoundAction::ConfirmBefore { .. } => "confirm-before",
    }
}

/// Carry out one of the user's bound commands on the focused pane — every action a
/// [`Keymap`](sprag_host::keymap::Keymap) can name, each through the mechanism this client already
/// had for it.
///
/// Counted in prose until R289 added the fifth, which is what a number kept in a comment does.
///
/// `detach-client` is the quit sink and not a session teardown: closing this window leaves the daemon
/// and the session running, which is what detaching MEANS in topology B (`sprag attach` connect-or-
/// spawns a daemon it never owns). `send-prefix` sends the PREFIX read off the live table, not the
/// key that triggered it — a user who binds it to `a` means `prefix a` to type `C-b`.
///
/// Nothing here repaints: a split's new pane and a focus move both reach the paint through the
/// channels that already carry them (the host announces the pane set, and a focus request re-derives
/// the ring), which is why the palette's `New pane` needs no repaint either.
pub(crate) fn perform(action: BoundAction, active: usize) {
    match action {
        BoundAction::DetachClient => pinion_core::use_quit_sink().request_quit(),
        BoundAction::SendPrefix => {
            let prefix = use_client_keys().prefix();
            // Whether the PTY could be given it is discarded on purpose. Elsewhere a `false` from
            // `send_key` means "unencodable, so fall through to the shell default"; here the key was
            // already decided to be this client's — the user typed the prefix twice — so there is
            // nothing to fall through to. A prefix the encoder cannot spell (`F13`) sends nothing,
            // which is the honest outcome of binding one.
            let _ = use_terminal()
                .slots
                .send_key(active, prefix.name(), prefix.mods());
        }
        // THE ONLY ACTION THAT SHOWS THIS CLIENT ITSELF, and so the only one whose whole effect is
        // a surface of its own: it reaches no daemon and moves nothing. The table is read from the
        // live client keys, so a user who has just edited their config is shown what they wrote.
        BoundAction::ListKeys => crate::keyhelp::show(use_client_keys().help()),
        // Reached only when `Ask::of` answered `None` — a daemon with an empty tree, which is a
        // question with nothing to ask about. The chooser's real path is the `Ask::Choose` arm in
        // `route_key`, exactly as the rename verbs' is.
        BoundAction::ChooseTree => {}
        BoundAction::SplitWindow { dir, before } => {
            use_terminal().slots.split(active, dir, before);
        }
        BoundAction::SelectNextPane => cycle_focus(active, true),
        // Unlike the cycle above, this one does NOT move the ring: it asks the daemon to move the
        // session's active pane, and `crate::active_pane`'s reconcile follows that onto a focus
        // request on the next pass. One path to "which pane the session is on" rather than two,
        // which is the same discipline the zoom below leans on.
        BoundAction::SelectPaneToward { dir } => use_terminal().slots.select_toward(dir),
        // The same shape one verb over, and for the same reason: the daemon resolves the direction
        // and announces the arrangement it produced, which is the channel the dock topology is
        // already projected from. Nothing here repaints.
        BoundAction::SwapPaneToward { dir } => use_terminal().slots.swap_toward(dir),
        // The same shape again, and the reason the DISTANCE travels rather than being applied here:
        // a cell becomes a share against the split's region under the ARBITRATED window, which is
        // derived from every attached client's report and not from this one's surface. A client
        // converting it would move the boundary a different distance than the user asked for
        // whenever another client is larger.
        BoundAction::ResizePaneToward { dir, cells } => {
            use_terminal().slots.resize_toward(dir, cells);
        }
        // The wire client re-reads the arrangement when the zoom moved anything, and the layout
        // mirror is what the dock topology is projected from — so this repaints through the channel
        // that already carries an arrangement change, exactly as the split above does.
        BoundAction::ZoomPane { on } => {
            use_terminal().slots.zoom_pane(active, on);
        }
        // The CASCADE is the daemon's (R309): closing the window's last pane ends the window, and
        // this client learns that the way it learns every set change — the host announces and the
        // mirror is re-read. So there is nothing to repaint here and nothing to decide: the answer
        // says how far it went, and a display client acts on the arrangement rather than on the
        // word.
        BoundAction::KillPane => {
            use_terminal().slots.close_pane(active);
        }
        // THE WINDOW LEVEL (R305). Nothing here repaints, for this function's standing reason: a
        // window change reaches the paint through the channels that already carry it — the host
        // announces the new pane set and the ring is re-derived — which is the same path the split
        // above relies on.
        BoundAction::NewWindow => {
            use_terminal().slots.new_window();
        }
        // The walk is the DAEMON's, exactly as the directional pane move above is: this asks, and
        // the answer arrives as the arrangement the host publishes.
        BoundAction::SelectWindow { ask } => {
            let slots = &use_terminal().slots;
            match ask {
                SelectWindowAsk::Named(window) => slots.select_window(&window),
                SelectWindowAsk::Step(step) => {
                    slots.select_window_toward(step);
                }
            }
        }
        // The CURRENT window, which is the only one a keystroke can mean — read off the same
        // mirror the tab bar draws from, where `current` is the fact the daemon publishes for it.
        BoundAction::KillWindow => {
            let slots = &use_terminal().slots;
            if let Some(window) = slots.windows().into_iter().find(|w| w.current) {
                slots.kill_window(&window.name);
            }
        }
        // NO window named, unlike the kill above it: the daemon resolves "the one I am on" under
        // the lock that performs the move. The kill reads the mirror because a kill's QUESTION
        // already named a window to the user and the act must match what they agreed to; nothing
        // was agreed to here.
        //
        // The outcome word is discarded here and NOT in the prompt arm beside it: this client
        // repaints its window strip off the daemon's own announcement, so a move that changed
        // nothing repaints nothing — where a user who answered a question is owed a sentence.
        BoundAction::MoveWindow { place } => {
            use_terminal().slots.move_window(None, &place);
        }
        // THE SESSION LEVEL (R314). The ring is the DAEMON's for the same reason the window ring
        // above it is, and more sharply: this client's `sessions` mirror is refreshed by a poll, so
        // a step resolved here could name a row that has moved — which is what the private
        // `SessionChord` table this replaces actually did, walking `session_neighbour` over the
        // mirror and then attaching BY NAME.
        //
        // The ASKING arm is absent here on the rule stated below: it arrives as an ANSWER.
        BoundAction::SwitchClient { ask } => {
            let slots = &use_terminal().slots;
            match ask {
                SwitchClientAsk::Step(step) => {
                    slots.switch_session_toward(step);
                }
                SwitchClientAsk::LastViewed => slots.switch_to_last_session(),
                // The ANSWERING form, which is what `sprag-tui` calls for this same arm. Both
                // discard the landing (a keystroke has nowhere to paint a refusal), but calling two
                // different methods for one action is how two frontends come to disagree — and the
                // non-answering one cannot tell a bad name from a good one even if a caller wanted
                // to know.
                SwitchClientAsk::Named(session) => {
                    slots.switch_session_named(&session);
                }
                SwitchClientAsk::Ask => {}
            }
        }
        // THE ACTIONS THAT ASK reach this function only through their own question being
        // ANSWERED, which is why they are not opened here: `route_key` calls `prompt::Ask::of`
        // before it performs anything, and a `confirm-before`'s yes re-enters this function with the
        // verb it guarded. Reached with an ask outstanding only if `Ask::of` answered `None` — a
        // `rename-pane` with no pane focused, which has no subject to rename.
        BoundAction::RenameWindow
        | BoundAction::RenameSession
        | BoundAction::RenamePane
        | BoundAction::MoveWindowBefore
        | BoundAction::ConfirmBefore { .. } => {}
    }
}

/// Route a focused keystroke to the **focused pane's** PTY. The roving-tabindex
/// gate maps `focused` to a pane tile ([`pane_index_of`]); a non-pane / absent
/// focus is a no-op (falls through to the shell default). A reserved
/// [`WindowChord`] ([`window_chord`]) acts on the window/layout and never reaches
/// the PTY (focus-cycle / scrollback / dock-toggle); a DISCRETE chord (dock-toggle /
/// focus-cycle) is dropped on an OS auto-repeat (`repeat`) so it acts once per press
/// (pinion R1071 / PINION-PR27 — a held `Ctrl+Shift+Enter` no longer dock-then-undocks),
/// while scrollback + PTY keys repeat normally. Otherwise the key + W3C modifiers
/// are SENT to the focused pane through the host client
/// ([`HostClient::send_key`](sprag_host::HostClient::send_key)), which encodes them
/// to PTY bytes via the shared host SSOT ([`sprag_host::send_key`]) — the same
/// key->PTY encoder the AI `scene/invoke` path uses (§2 #2; encoding is sprag's,
/// R2.6). Topology B: the GUI's keyboard is a client SEND, not a mutation of its
/// own paint scene. An unencodable key returns `false`, so it falls through rather
/// than injecting nothing. Returning `true` for the encodable keys swallows
/// Escape/Tab from the shell's quit/traverse defaults so a full-screen TUI receives them.
pub(crate) fn route_key(
    scene: &mut Scene,
    focused: Option<&str>,
    key: &str,
    modifiers: Modifiers,
    repeat: bool,
) -> bool {
    // The user's prefix mode is taken out HERE, in the first statement, before anything looks at what
    // the key is or where it is going. The mode is ONE KEY LONG whatever that key turns out to be, and
    // five of the surfaces below consume a keystroke before the pane is even resolved — so a prefix
    // armed in a pane must not survive a character typed into a find field. One place that can end the
    // mode is one place that can forget to; the single re-arm is in `keys::ClientKeys::route`.
    let prefixed = use_client_keys().take();
    // The destructive-command prompt, FIRST and gated on the prompt being UP rather than on what holds
    // focus: while the client is asking whether to destroy something, no key may reach anything else —
    // not a pane behind the scrim, not the palette row that armed it. Every other route below is keyed
    // on the focused tag, which is right for a surface that can share the screen; this one cannot, and
    // a focus gate would leak the keystroke in the one case where that is worst — including when there
    // is NO focus at all, which is why this precedes the `focused` destructure rather than following
    // it. Answering is `Enter` on the CHOSEN button, which starts on Cancel (see [`crate::confirm`]).
    if crate::confirm::handle_key(key) {
        return true;
    }
    // The NAME prompt, on the same terms and for the same reason (R306): while this client is
    // asking for a name, no key may reach a pane behind the scrim — including when there is no
    // focus at all, which is why this precedes the `focused` destructure rather than following it.
    // It is BELOW the confirmation because a destructive yes is the more dangerous question; the
    // two are never up together, so the order is a guarantee rather than a mechanism.
    if crate::prompt::is_open() {
        return crate::prompt::handle_key(scene, key, modifiers);
    }
    // THE KEY TABLE, on the same terms as the two above and for the same reason: while this client
    // is showing what its keys do, no key may reach a pane behind the scrim. It is BELOW both
    // questions because neither can be armed while it is up — it swallows every key — so the order
    // is a guarantee rather than a mechanism, exactly as it is for the pair above.
    if crate::keyhelp::is_open() {
        return crate::keyhelp::handle_key(key, modifiers, (crate::WINDOW_W, crate::WINDOW_H));
    }
    // THE CHOOSER, on the same terms as the three above and for the same reason: while this client
    // is showing a person where they can go, no key may reach a pane behind the scrim. It is BELOW
    // the two questions because neither can be armed while it is up — it swallows every key — and
    // below the key table only because that one is the older surface; the four are never up
    // together, so the order is a guarantee rather than a mechanism.
    if crate::chooser::is_open() {
        return crate::chooser::handle_key(key, modifiers);
    }
    let Some(tag) = focused else {
        return false;
    };
    // The find bar (find-in-scrollback): with its field focused, every key belongs to the SEARCH —
    // typing edits the needle, Enter steps matches, Escape closes — so it is dispatched before the
    // pane gate below drops a non-pane focus. The `scene` is threaded this far for exactly this: a
    // field edit is delivered through the field's own External (pinion's `forward_key_to_field`),
    // which needs the model scene; every other route here is a client SEND to the host.
    if crate::find::is_find_focus(tag) {
        return crate::find::handle_key(scene, key, modifiers);
    }
    // The command palette: with its query field focused every key belongs to the palette — typing
    // filters, the arrows walk the rows, Enter runs, Escape closes — and none of them may reach a
    // pane behind the modal scrim. Dispatched before the pane gate for the same reason the find
    // bar's is, and before the palette CHORD below so a `Ctrl+Shift+P` typed into an open palette
    // is the field's to swallow rather than a second open.
    if crate::palette::is_palette_focus(tag) {
        return crate::palette::handle_key(scene, key, modifiers);
    }
    // ...and the chord that OPENS it, checked before the pane gate below so the palette is reachable
    // from any focus, not only from a pane. It captures whatever pane is focused NOW (`None` when
    // the focus is elsewhere), which is the pane its pane-commands will act on.
    if palette_chord(key, modifiers) {
        crate::palette::open(pane_index_of(tag));
        return true;
    }
    // The session rail (R179): with the sidebar focused — its `tablist` (the list's single Tab
    // stop) or a footer button — route the key to the sidebar keyboard model (rove the cursor /
    // switch / arm-kill / confirm), NOT the pane PTY. Checked BEFORE the pane focus gate below,
    // which drops a non-pane focus to `false`. The `sprag` CLI's session control is unaffected.
    if crate::stabs::is_sidebar_focus(tag) {
        return crate::stabs::handle_sidebar_key(tag, key, repeat, &use_terminal().slots);
    }
    let Some(active) = pane_index_of(tag) else {
        return false;
    };
    // THE USER'S KEYMAP, and it is consulted here for two reasons that decide the position exactly.
    //
    // AFTER the pane gate above: a keystroke a text field or the session rail owns is not CONTESTED.
    // The prefix exists because a pane's child owns every key; a field owns its keys outright, and
    // arming a one-key mode there would eat the next character out of a user's search needle. Every
    // action a binding can name acts on the focused pane anyway.
    //
    // SLICE 4 MADE THAT POSITION LOAD-BEARING RATHER THAN MERELY RIGHT. A prefix binding could not
    // reach a field in the first place — it needs a prefix keystroke, and the mode is taken out
    // above. A ROOT binding is on a bare key, so with the lookup any higher, a user who bound `F5`
    // and then typed `F5` into a search needle would get a split instead of a search.
    //
    // BEFORE every built-in chord below: a table the user wrote outranks one this binary spells. A
    // prefix rebound onto `Ctrl+Shift+C` has to be reachable, and after the prefix that same chord is
    // an unbound command key, which tmux swallows. In the steady state nothing is taken — the prefix
    // space and the reserved `Ctrl+Shift+*` space are disjoint until a user puts one on top of the
    // other, and then that is what they asked for. A root binding a user puts ON one of those chords
    // takes it, which is the same answer and the same reason.
    match use_client_keys().route(prefixed, key, to_input_mods(modifiers)) {
        // The repeat window rides on `Routed::next`, which `ClientKeys::route` already stored — a
        // caller reading `again` here would be a second author of the mode transition.
        Routed::Act { action, again } => {
            // AN OS AUTO-REPEAT IS NOT A SECOND PRESS unless the binding said so. `-r` is exactly
            // the statement that holding this key is meaningful (tmux marks its arrows and its
            // resizes and nothing else), so a binding without it acts once per press — which is
            // what the GUI's private chord table did for its three session keys before R314 put
            // them in this vocabulary, and what a ROOT binding needs most: held down, it would walk
            // the session ring at the keyboard's repeat rate.
            //
            // `sprag-tui` cannot make this distinction and does not try: a terminal delivers an
            // auto-repeat as an ordinary keystroke, so there is nothing to tell apart. tmux on a
            // terminal is in the same position.
            if repeat && again.is_none() {
                crate::diag::chord(action_label(&action), "drop-repeat", active);
                return false;
            }
            crate::diag::chord(action_label(&action), "act", active);
            // An action that cannot be carried out without an ANSWER opens a question instead of
            // acting, and WHICH ones those are is `Ask::of`'s decision alone — asked here for every
            // action, so this client and `sprag-tui` cannot come to different conclusions about
            // whether a verb needs asking about.
            match sprag_host::prompt::Ask::of(
                &action,
                use_terminal().slots.host(),
                use_terminal().slots.pane_at(active),
            ) {
                Some(sprag_host::prompt::Ask::Line { subject, seed }) => {
                    crate::prompt::open(subject, &seed);
                }
                Some(sprag_host::prompt::Ask::Choose { pick }) => {
                    crate::chooser::show(*pick);
                }
                Some(ask @ sprag_host::prompt::Ask::Confirm { .. }) => {
                    let sprag_host::prompt::Ask::Confirm { action, .. } = &ask else {
                        unreachable!("matched Confirm above")
                    };
                    crate::confirm::arm_bound(action, active, &ask);
                }
                None => perform(action, active),
            }
            return true;
        }
        // The prefix itself, and a command key bound to nothing: both are consumed. Passing an
        // unbound one on would run something in the pane that the user — who had just addressed this
        // client — never asked for.
        Routed::Prefix | Routed::Swallow => return true,
        Routed::ToPane => {}
    }
    // Clipboard chords (R139) act on the selection / clipboard, not the PTY: copy the
    // active selection to CLIPBOARD, or paste CLIPBOARD into the focused pane. Consumed
    // either way so `Ctrl+Shift+C/V` never reach the shell (Ctrl+C there is SIGINT).
    // `Ctrl+Shift+F` opens the find bar on the focused pane — the terminal-convention `Shift`
    // variant of the browser `Ctrl+F`, because bare `Ctrl+F` is a PTY key (readline forward-char,
    // vim page-down) that must keep reaching the child. Consumed here so it never does both.
    if find_chord(key, modifiers) {
        crate::find::open(active);
        return true;
    }
    if let Some(chord) = clipboard_chord(key, modifiers) {
        match chord {
            ClipboardChord::Copy => {
                crate::selection::copy_selection();
            }
            ClipboardChord::Paste => {
                let _ = crate::selection::paste_clipboard(active);
            }
        }
        return true;
    }
    // The SESSION chords are gone from here (R314). `Ctrl+Shift+PageUp` / `PageDown` are ROOT
    // bindings of the one vocabulary now, so they were already taken by the keymap route above —
    // where `sprag list-keys` can name them, a config file can unbind them, and `sprag-tui` has
    // them too. They still reach the session before `window_chord` sees them, because that route
    // runs first. `Ctrl+Shift+L` is NOT among them and its verb lives on `prefix L`: this
    // vocabulary cannot tell `C-S-L` from `C-l`, so binding it would take the shell's
    // clear-screen from every pane (see `BoundAction::SwitchClient`'s default table).
    // Reserved window chords act on the layout, not the PTY.
    if let Some(chord) = window_chord(key, modifiers) {
        // Discrete chords (dock-toggle, focus-cycle) act once per press: drop an OS
        // auto-repeat re-send (pinion R1071 / PINION-PR27 toggle-class contract), or a
        // held `Ctrl+Shift+Enter` dock-then-undocks in the multi-window state. Scrollback
        // is continuous, so a held `Shift+PageUp` keeps scrolling.
        if repeat && !matches!(chord, WindowChord::Scroll) {
            crate::diag::chord(chord.label(), "drop-repeat", active);
            return false;
        }
        crate::diag::chord(chord.label(), "act", active);
        match chord {
            WindowChord::CycleFocus => cycle_focus(active, key == "PageDown"),
            WindowChord::Scroll => scroll_view(active, key),
            WindowChord::ToggleDock => {
                crate::dock::toggle_pane_floating(active);
                // Keep typing in the same pane: focus it (a no-op on undock —
                // already focused; correct on dock-back so a dropped window cannot
                // strand focus).
                pinion_core::focus_request::request(pane_tag(active));
            }
            WindowChord::JumpPrompt => scroll_to_prompt(active, key),
        }
        return true;
    }
    // Any other key is a live interaction with the focused pane: snap its view to
    // the live bottom (offset_y == max), then SEND it to that pane through the host
    // client (topology B: the GUI's keyboard is a client SEND, not a mutation of its
    // own paint scene — the same key->PTY encoder the AI `scene/invoke` path uses,
    // and over the wire this becomes an RPC send to the host). An unencodable key
    // returns `false`, so it falls through to the shell default rather than swallowing.
    let scroll = crate::scrollbar::use_pane_scroll(active);
    scroll.scroll_to(0, scroll.max().1);
    // A typed key clears any mouse selection (R139): its inverted band would go stale
    // as the pane's content changes, and the text is already on PRIMARY by now
    // (select-to-copy). No-op when nothing is selected.
    crate::selection::clear();
    use_terminal()
        .slots
        .send_key(active, key, to_input_mods(modifiers))
}

/// Map pinion's key [`Modifiers`] to the encoder's [`sprag_input::Modifiers`]:
/// pinion's `meta` (Cmd / Super / Win) is the encoder's `sup` (the only rename).
pub(crate) fn to_input_mods(m: Modifiers) -> sprag_input::Modifiers {
    sprag_input::Modifiers {
        ctrl: m.ctrl,
        alt: m.alt,
        shift: m.shift,
        sup: m.meta,
    }
}

/// Route an IME composition (Hangul / CJK / any composed input) to the **focused
/// pane**: mirror the in-progress preedit into that pane's [`use_preedit`] overlay
/// Signal and write only the committed text to its PTY. See
/// `sprag_grid::overlay_preedit` for why a terminal must render the preedit itself
/// (winit + XIM) and the display-only contract.
///
/// - [`Start`](CompositionEvent::Start) / [`Cancel`](CompositionEvent::Cancel):
///   clear the focused pane's overlay (begin / abort).
/// - [`Update`](CompositionEvent::Update): mirror the preedit text into that
///   pane's overlay — NOT written to the PTY.
/// - [`Commit`](CompositionEvent::Commit): clear the overlay and write the text
///   **literally** to the focused pane through the host client
///   ([`HostClient::send_text`](sprag_host::HostClient::send_text)) — the same
///   text->PTY seam the AI peer drives, bypassing the key encoder. An empty Commit
///   is the cancel-shaped end (clearing is the whole job; no write).
///
/// Focus-gated to a pane tile like [`route_key`]. Returning `true` reports the
/// event handled AND — because `view` subscribes to [`use_preedit`] — arms a
/// repaint via pinion's R705.1 reactive-dirty bridge, so the composition repaints
/// live. (`WidgetView::ime_caret_rect`, Hanja-candidate positioning, is still
/// unwired — polish, not shown during plain Hangul.)
pub(crate) fn route_composition(focused: Option<&str>, event: &CompositionEvent) -> bool {
    // Trace every composition reaching the binding (before the focus gate) so a
    // Hangul-then-space stream is greppable against the key trace — a `commit` with
    // no following space `key_in` pins an IME key swallowed upstream.
    let (kind, text) = match event {
        CompositionEvent::Start => ("start", ""),
        CompositionEvent::Update(t) => ("update", t.as_str()),
        CompositionEvent::Commit(t) => ("commit", t.as_str()),
        CompositionEvent::Cancel => ("cancel", ""),
        _ => ("other", ""),
    };
    crate::diag::composition(kind, text, focused);
    let Some(tag) = focused else {
        return false;
    };
    let Some(active) = pane_index_of(tag) else {
        return false;
    };
    // Composition happens at the live prompt, so snap the focused pane's
    // scrollback view to the live bottom once for any composition activity — one
    // site, and it also covers a Commit that arrives without a preceding Start.
    let scroll = crate::scrollbar::use_pane_scroll(active);
    scroll.scroll_to(0, scroll.max().1);
    match event {
        // Begin / abort: clear any (stale) overlay. Update carries the live text.
        CompositionEvent::Start | CompositionEvent::Cancel => {
            use_preedit(active).set(String::new());
            true
        }
        // Preedit progresses (ㅎ -> 하 -> 한): mirror it into the pane's overlay.
        CompositionEvent::Update(text) => {
            use_preedit(active).set(text.clone());
            true
        }
        // Finished: clear the overlay, then write the literal committed text to the
        // focused pane through the host client (the same text->PTY seam the AI
        // `scene/invoke` path uses, bypassing the key encoder). An empty commit is a
        // no-op (send_text no-ops it).
        CompositionEvent::Commit(text) => {
            use_preedit(active).set(String::new());
            if !text.is_empty() {
                // The committed-text write. A PTY-write failure has nothing for an
                // IME commit to fall through to (unlike a keystroke, which returns
                // false to the shell), so the result is intentionally discarded —
                // made explicit now that `send_text` is `#[must_use]`.
                let _ = use_terminal().slots.send_text(active, text);
            }
            true
        }
        // `CompositionEvent` is `#[non_exhaustive]`: a future variant is
        // unhandled (falls through to the shell default), not silently consumed.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalViewer;
    use crate::terminal::seed_terminal;
    use pinion_core::Scene;
    use pinion_core::WidgetCore;
    use pinion_core::scene::ContainerNode;
    use sprag_host::Host;
    use sprag_terminal::{CommandBuilder, PanePtyHandle};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// A long-lived `cat` pane (echoes stdin, keeps the PTY open across keys).
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// Poll a handle's row 0 until it contains `needle` or the deadline passes.
    fn wait_for_row0(handle: &PanePtyHandle, needle: &str) -> String {
        wait_for_row0_where(handle, |row| row.contains(needle))
    }

    /// Poll a handle's row 0 until `holds` is satisfied or the deadline passes, then answer the row.
    ///
    /// The predicate form exists because a keystroke's arrival is not always spellable as a substring:
    /// a control byte's echo is the child's termios' business, so "both `%` arrived" is a COUNT rather
    /// than a `"%%"` — and waiting on the count is what makes the assertion race-free (waiting on the
    /// first `%` would read the row before the second arrived).
    fn wait_for_row0_where(handle: &PanePtyHandle, holds: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        let mut row0 = String::new();
        while start.elapsed() < Duration::from_secs(5) {
            row0 = handle.with_screen(|screen| screen.row_text(0));
            if holds(&row0) {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        row0
    }

    /// A temporary `config.toml` for the keymap tests, removed on drop.
    ///
    /// A FILE rather than a seeded table, because the file is the live table: `sprag bind-key` edits
    /// it and a running client is supposed to notice. A test that seeded a `Keymap` directly could not
    /// tell a client that re-reads from one that read once at boot — the whole claim of slice 2.
    ///
    /// Its own directory per test, so `$XDG_CONFIG_HOME` is never touched: the environment is
    /// process-global and this crate's tests run in parallel, so pointing it anywhere would have
    /// siblings reading whatever the last writer left.
    /// Moved to [`crate::keys::test_support`] when the palette's hint column became a second
    /// consumer (R308) — one fixture, so two suites cannot come to different ideas of what "a known
    /// keymap" is.
    use crate::keys::test_support::Config;

    /// A quit sink that counts, seeded into pinion's provider slot so `detach-client` is observable.
    #[derive(Debug)]
    struct CountingQuit(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl pinion_core::QuitSink for CountingQuit {
        fn request_quit(&self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Seed the quit sink for this owner and hand back its counter.
    fn counting_quit(owner: &Owner) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink: std::sync::Arc<dyn pinion_core::QuitSink> =
            std::sync::Arc::new(CountingQuit(std::sync::Arc::clone(&count)));
        pinion_core::QUIT_SINK.provide(owner, sink);
        count
    }

    /// Ctrl, as the keymap tests spell it.
    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    /// A two-pane `cat` host, so a keystroke that reaches a PTY is observable and a split is
    /// countable.
    fn two_cats() -> (Host, PanePtyHandle) {
        let host = Host::new((40, 6));
        let id0 = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .expect("pane 0");
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .expect("pane 1");
        let handle = host.pane_handle(id0).expect("pane 0 handle");
        (host, handle)
    }

    /// How many panes this client can see, reconciled first — the read a frame does before it paints,
    /// so a pane born on the host is counted the way the real client counts it.
    fn panes() -> usize {
        let slots = &use_terminal().slots;
        let _ = slots.reconcile();
        slots.occupied_slots().len()
    }

    /// Deliver one keystroke to pane `pane` through the full public path.
    fn press(pane: usize, key: &str, mods: Modifiers) -> bool {
        let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
        TerminalViewer::apply_key(&mut scene, Some(pane_tag(pane)), key, mods)
    }

    /// **THE SLICE: the GUI acts on the user's own prefix table.** `prefix = "C-a"` in a file this
    /// client never wrote, and `C-a` then `%` splits the focused pane.
    ///
    /// The FIRST assertion is what discriminates: a bare `%` before any prefix must reach the PANE and
    /// split nothing, so a client that treated every `%` as a split cannot pass. The second is the
    /// other half — `C-b`, the DEFAULT prefix, must arm nothing once the file has moved it, so a
    /// client holding `Keymap::default()` cannot pass either.
    ///
    /// REVERT-PROOF (both measured): route on `Keymap::default()` and `C-a` reaches the pane while
    /// `C-b %` splits — the two assertions fail in opposite directions.
    #[test]
    fn the_users_prefix_table_splits_the_focused_pane() {
        let (host, pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("[options]\nprefix = \"C-a\"\n");
            seed_terminal(host);
            let before = panes();

            // A bare command key is the program's: it reaches the PTY and divides nothing.
            assert!(press(0, "%", Modifiers::default()));
            assert_eq!(
                panes(),
                before,
                "a `%` with no prefix before it is a character, not a split",
            );

            // The DEFAULT prefix arms nothing, because the file moved it.
            assert!(press(0, "b", ctrl()));
            assert!(press(0, "%", Modifiers::default()));
            assert_eq!(
                panes(),
                before,
                "`C-b` is not this user's prefix, so `%` after it is still a character",
            );

            // ...and the prefix the FILE names does arm.
            assert!(press(0, "a", ctrl()));
            assert!(press(0, "%", Modifiers::default()));
            assert_eq!(panes(), before + 1, "`prefix %` divided the focused pane",);
        });
        // The two `%` meant for the shell were DELIVERED; the third was the client's and was not. A
        // count rather than `"%%"` because the `C-b` between them echoes as the child pleases — and
        // waiting on the count is what keeps this from reading the row before the second one landed.
        let row0 = wait_for_row0_where(&pane0, |row| row.matches('%').count() >= 2);
        assert_eq!(
            row0.matches('%').count(),
            2,
            "both unprefixed keys reached the pane and the prefixed one did not: {row0:?}",
        );
    }

    /// **A shifted character reaches its binding here, where a real keyboard sets the SHIFT flag the
    /// synthesized presses above never do.**
    ///
    /// R306 measured the gap and it was a shipped defect: winit reports `Shift+5` as the W3C key
    /// `"%"` with `shift_key()` ALSO set (`winit_modifiers_to_pinion`), while `sprag-tui` reads the
    /// raw pty byte and reports `"%"` with no modifier at all — so an exact modifier comparison
    /// matched the terminal client and not this one, and `prefix %` (a tmux default sprag has
    /// shipped since the keymap existed) did nothing in the GUI on a real keyboard.
    ///
    /// Every existing test missed it for the same reason: [`press`] and the pixel smoke's
    /// `scene/key` both synthesize the character WITHOUT the flag a keyboard sets. So this test
    /// presses it the way winit does, which is the whole point of it.
    ///
    /// REVERT-PROOF: restore the exact `self.mods == mods` comparison in `KeySpec::matches` and the
    /// split assertion fails while the sibling above still passes.
    #[test]
    fn a_shifted_character_reaches_its_binding_as_a_keyboard_sends_it() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("[options]\nprefix = \"C-a\"\n");
            seed_terminal(host);
            let before = panes();

            // `Shift+5` as winit delivers it: the character, AND the modifier bit.
            let shifted = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            assert!(press(0, "a", ctrl()));
            assert!(press(0, "%", shifted));
            assert_eq!(
                panes(),
                before + 1,
                "`prefix %` divides the pane when the shift flag rides along, as a keyboard sends it",
            );

            // THE CONTROL, which is what keeps the fix from being "ignore shift everywhere": a
            // NAMED key's shift is a real modifier, so `S-ArrowLeft` (swap) and `ArrowLeft`
            // (select) must stay two different bindings. Asserted through the table rather than
            // through a pane count, because both of those act on the arrangement.
            let keys = use_client_keys();
            let armed = sprag_host::keymap::PrefixMode::AfterPrefix;
            let bare = keys.route(armed, "ArrowLeft", sprag_input::Modifiers::default());
            let with_shift = keys.route(armed, "ArrowLeft", to_input_mods(shifted));
            assert_ne!(
                bare, with_shift,
                "a NAMED key's shift is a modifier a user really holds, and still tells two \
                 bindings apart",
            );
        });
    }

    /// **A `-n` binding acts in this frontend too, with no prefix — and the key never reaches the
    /// PTY.**
    ///
    /// Both frontends route through one `Keymap::route`, so the routing itself is settled in
    /// `sprag-host`. What is GUI-specific, and what this test is for, is WHERE the lookup sits: the
    /// user's table is consulted AFTER the pane gate, so a root binding fires in a pane and cannot
    /// fire while a text field owns the keyboard.
    /// [`a_root_binding_does_not_fire_while_a_field_holds_the_keyboard`] asserts the second half;
    /// this asserts the first.
    ///
    /// REVERT-PROOF: drop the root lookup and `F5` becomes a keystroke — the split never happens and
    /// the pane receives a key the user had bound to a command.
    #[test]
    fn a_root_binding_splits_the_focused_pane_with_no_prefix() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded(
                "[[bind]]\nkey = \"F5\"\naction = \"split-window -h\"\ntable = \"root\"\n",
            );
            seed_terminal(host);
            let before = panes();

            // A key nobody bound is still the program's, so a client that split on everything cannot
            // pass this.
            assert!(press(0, "F6", Modifiers::default()));
            assert_eq!(panes(), before, "an unbound function key is a keystroke");

            assert!(press(0, "F5", Modifiers::default()));
            assert_eq!(
                panes(),
                before + 1,
                "the root binding acted with no prefix at all",
            );
        });
    }

    /// **A root binding does not fire while a text field holds the keyboard**, and this is the half
    /// of `-n` that only the GUI has to answer.
    ///
    /// A prefix binding could not do this damage: it needs a prefix keystroke first, and the mode is
    /// taken out before any surface is consulted. A ROOT binding is on a bare key, so a user who
    /// bound `F5` and then typed `F5` into a search needle would have their client split a pane
    /// instead of searching — the keystroke a field owns is not CONTESTED, which is exactly the
    /// argument that put the keymap after the pane gate in the first place.
    ///
    /// Driven with the find field's own tag rather than an invented one, so what is asserted is the
    /// real focus a user has while typing a search.
    ///
    /// REVERT-PROOF: move the keymap lookup above the `find` / `palette` / sidebar routes and this
    /// splits.
    #[test]
    fn a_root_binding_does_not_fire_while_a_field_holds_the_keyboard() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded(
                "[[bind]]\nkey = \"F5\"\naction = \"split-window -h\"\ntable = \"root\"\n",
            );
            seed_terminal(host);
            let before = panes();
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // Whether the FIELD consumed it is `find`'s business; the claim here is only that the
            // keymap did not.
            let _ = TerminalViewer::apply_key(
                &mut scene,
                Some(crate::find::FIND_FIELD_TAG),
                "F5",
                Modifiers::default(),
            );
            assert_eq!(
                panes(),
                before,
                "a key the search field owns is not the keymap's to take",
            );
        });
    }

    /// A rebind reaches a RUNNING window: the file is edited under a client that is already up, and
    /// the very next keystroke follows the new table.
    ///
    /// This is `sprag bind-key`'s claim on this frontend — and `source-file`'s, since an editor writes
    /// the same bytes.
    ///
    /// REVERT-PROOF: drop the `refresh` in `ClientKeys::route` (read the file once at boot) and the
    /// second half fails — `prefix k` stays unbound forever while `sprag list-keys` prints it.
    ///
    /// The key is `k` and not `c` because R305 gave `c` a DEFAULT (`new-window`): this test needs a
    /// key the shipped table does not mention, and one that quietly acquired a meaning would make
    /// the first assertion pass for the opposite reason.
    #[test]
    fn an_edit_to_the_file_reaches_a_running_client() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (config, _keys) = Config::seeded("[options]\nprefix = \"C-a\"\n");
            seed_terminal(host);
            let before = panes();

            // `k` is bound to nothing yet: swallowed after the prefix, and nothing happens.
            assert!(press(0, "a", ctrl()));
            assert!(press(0, "k", Modifiers::default()));
            assert_eq!(panes(), before);

            config.write(
                "[options]\nprefix = \"C-a\"\n\n[[bind]]\nkey = \"k\"\naction = \"split-window -v\"\n",
            );
            assert!(press(0, "a", ctrl()));
            assert!(press(0, "k", Modifiers::default()));
            assert_eq!(panes(), before + 1, "the edit landed with no reattach",);
        });
    }

    /// **The prefix mode is ONE KEY LONG, including when another surface took that key.**
    ///
    /// Arm the prefix in a pane, then deliver a keystroke that never reaches the pane gate at all (no
    /// focus). The mode must be spent by it, so the next `d` is a letter for the shell rather than a
    /// detach nobody asked for.
    ///
    /// REVERT-PROOF: take the mode inside the pane block instead of in `route_key`'s first statement
    /// and this fails — the stale mode survives the unrelated keystroke and eats the next one.
    #[test]
    fn the_mode_does_not_survive_a_keystroke_that_went_elsewhere() {
        let (host, pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("");
            let quits = counting_quit(&Owner::current().expect("owner"));
            seed_terminal(host);

            assert!(press(0, "b", ctrl()), "the prefix is armed");
            // A keystroke with NO focus: `route_key` falls through to the shell default without
            // resolving a pane. It still ends the mode.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(!TerminalViewer::apply_key(
                &mut scene,
                None,
                "x",
                Modifiers::default()
            ));
            assert!(press(0, "d", Modifiers::default()));
            assert_eq!(
                quits.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "`d` was a letter, not the detach a stale prefix would have made it",
            );
        });
        assert!(
            wait_for_row0(&pane0, "d").contains('d'),
            "and it reached the pane",
        );
    }

    /// `detach-client` pulls the shell's quit sink — the window goes and the daemon does not, which is
    /// what detaching means in topology B.
    ///
    /// Also the negative half: the keystroke reaches no PTY, so a program in the pane never sees the
    /// `d`.
    #[test]
    fn detach_client_pulls_the_quit_sink_and_no_pty() {
        let (host, pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("");
            let quits = counting_quit(&Owner::current().expect("owner"));
            seed_terminal(host);
            assert!(press(0, "b", ctrl()));
            assert!(press(0, "d", Modifiers::default()));
            assert_eq!(quits.load(std::sync::atomic::Ordering::SeqCst), 1);
            // A SENTINEL after it, so the negative below is not a race dressed up as a sleep: once `z`
            // has echoed, anything the `d` was going to deliver has had its turn.
            assert!(press(0, "z", Modifiers::default()));
        });
        let row0 = wait_for_row0(&pane0, "z");
        assert!(
            !row0.contains('d'),
            "the command key was the client's, so the shell never saw it: {row0:?}",
        );
    }

    /// `send-prefix` types the PREFIX into the pane — the only way a program that binds the prefix key
    /// can still receive it.
    ///
    /// The prefix here is `!`, printable on purpose: a control character's echo depends on the child's
    /// termios, and the claim under test is which KEY was sent, not how a shell renders it. A printable
    /// prefix is also a table no default could have produced, so the assertion can only pass if the
    /// file was read.
    #[test]
    fn send_prefix_types_the_prefix_the_file_names() {
        let (host, pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("[options]\nprefix = \"!\"\n");
            seed_terminal(host);
            assert!(press(0, "!", Modifiers::default()), "arms");
            assert!(press(0, "!", Modifiers::default()), "and sends one through");
            // The sentinel makes the COUNT below race-free and discriminating in both directions: a
            // client with no prefix would have delivered two, one that swallowed the self-send none.
            assert!(press(0, "z", Modifiers::default()));
        });
        let row0 = wait_for_row0(&pane0, "z");
        assert_eq!(
            row0.matches('!').count(),
            1,
            "the pane received the prefix exactly once: {row0:?}",
        );
    }

    /// `select-pane -t :.+` moves focus on, through the same
    /// [`focus_request`](pinion_core::focus_request) the `Ctrl+PageDown` chord uses — so the framework
    /// focus ring and the next keystroke's routing both follow it.
    #[test]
    fn select_next_pane_requests_the_siblings_focus() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("");
            seed_terminal(host);
            let _ = pinion_core::focus_request::drain(); // clear any stale request
            assert!(press(0, "b", ctrl()));
            assert!(press(0, "o", Modifiers::default()));
            assert_eq!(
                pinion_core::focus_request::drain().as_deref(),
                Some(pane_tag(1)),
                "focus was requested for the next pane",
            );
        });
    }

    /// **A directional key moves the SESSION's pane and requests no focus of its own** — the arm's
    /// whole difference from the cycle above, asserted as both halves.
    ///
    /// `select-pane -t :.+` is resolved by this client and lands as a `focus_request`;
    /// `select-pane -L` is resolved by the HOST, and the ring follows through
    /// [`crate::active_pane`]'s reconcile on the next frame. A client that answered a direction with
    /// `cycle_focus` would drain a request here and move nothing on the host — which is exactly what
    /// the two assertions below are, one each.
    ///
    /// The EDGES are what make the middle mean something: two lefts settle on the leftmost pane and
    /// stay, so a direction is not a cycle. Pane 0 is the leftmost because
    /// [`LayoutTree::append_pane`](sprag_terminal::LayoutTree::append_pane) puts each birth at the
    /// rightmost position, so spawn order IS left-to-right here.
    #[test]
    fn a_directional_key_moves_the_hosts_pane_and_requests_no_focus() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("");
            seed_terminal(host);
            let _ = pinion_core::focus_request::drain();
            let _ = panes();
            let slots = &use_terminal().slots;
            let (left, right) = (slots.id(0), slots.id(1));
            assert!(
                left.is_some() && left != right,
                "the fixture needs two distinct panes: {left:?} / {right:?}",
            );

            // The fixture's start, ASSERTED rather than assumed — and the reason the moves below
            // run right-then-left rather than the other way round. A first press that confirmed a
            // state already in force would be vacuous, which is what the first draft of this test
            // was: two lefts against a session already on the leftmost pane pass over an arm that
            // does nothing at all. The revert-proof said so.
            assert_eq!(
                slots.active_pane(),
                left,
                "two spawns leave the session on the first pane",
            );

            // Each press either MOVES a state the one before it established, or holds an EDGE the
            // one before it reached. No assertion here is true of the state that preceded it.
            let steps = [
                ("ArrowRight", right, "right crosses to the second pane"),
                ("ArrowRight", right, "the right edge is quiet, not a wrap"),
                ("ArrowLeft", left, "left crosses back"),
                ("ArrowLeft", left, "and the left edge is quiet too"),
            ];
            for (key, want, why) in steps {
                assert!(press(0, "b", ctrl()));
                assert!(press(0, key, Modifiers::default()));
                assert_eq!(slots.active_pane(), want, "{why}");
            }

            assert_eq!(
                pinion_core::focus_request::drain(),
                None,
                "a directional key asks the HOST and lets the reconcile move the ring",
            );
        });
    }

    /// **The user's table outranks a chord this binary spells.** A prefix rebound onto `Ctrl+Shift+C`
    /// arms the mode instead of copying, and the command key after it is honoured.
    ///
    /// That ordering is the point: a keymap that lost to six hardcoded chords would be a keymap with
    /// six holes in it that no `list-keys` output could show. Nothing is taken in the steady state —
    /// the two spaces are disjoint until a user puts one on top of the other.
    ///
    /// REVERT-PROOF: move the keymap below the clipboard chord and `Ctrl+Shift+C` copies while the
    /// prefix becomes unreachable, so the detach never happens.
    #[test]
    fn the_users_prefix_outranks_a_built_in_chord() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, _keys) = Config::seeded("[options]\nprefix = \"C-S-c\"\n");
            let quits = counting_quit(&Owner::current().expect("owner"));
            seed_terminal(host);
            let mods = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            assert!(press(0, "C", mods), "the rebound prefix armed");
            assert!(press(0, "d", Modifiers::default()));
            assert_eq!(
                quits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "so the command key after it was the client's",
            );
        });
    }

    /// A config this client cannot use leaves it with a WORKING table and a report to show, because a
    /// window has no screen to fail on and a keymap error nobody can see is a keymap error nobody
    /// fixes.
    ///
    /// The report is what the command palette paints beside a broken project config
    /// ([`crate::palette`]); the DEFAULTS are what keeps the window usable in the meantime.
    #[test]
    fn a_broken_config_leaves_the_defaults_and_a_report() {
        let (host, _pane0) = two_cats();
        let owner = Owner::new();
        owner.run(|| {
            let (_config, keys) = Config::seeded(
                "[options]\nprefix = \"C-a\"\n\n[[bind]]\nkey = \"c\"\naction = \"kill-server\"\n",
            );
            let quits = counting_quit(&Owner::current().expect("owner"));
            seed_terminal(host);
            let report = keys.report().expect("a broken config reports why");
            assert!(
                report.contains("config.toml") && report.contains("kill-server"),
                "the report names the file and the line to fix: {report:?}",
            );
            // ASKED TWICE, because asking is what cleared it. The report path re-reads the file, and
            // an unchanged file has no news — so a holder that treated "nothing changed" as "nothing
            // is wrong" showed the error on the first palette open and nothing on the second. The
            // verdict now lives on the file, which is the only place that knows whether it looked.
            assert_eq!(
                keys.report(),
                Some(report),
                "and asking again does not clear a file that is still broken",
            );
            // The DEFAULT table is in force, so `C-b d` still detaches — the file's `C-a` is not.
            assert!(press(0, "b", ctrl()));
            assert!(press(0, "d", Modifiers::default()));
            assert_eq!(quits.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    /// The report describes the file as it is NOW: fixing the typo clears it.
    ///
    /// The palette is the only surface that shows it, and while the palette is OPEN its own field holds
    /// the keyboard — so no keystroke can reach a pane to trigger a re-read. Without the re-read on the
    /// report path, a user who fixed their config would keep being told it was broken.
    ///
    /// REVERT-PROOF: drop the `reread` in `ClientKeys::report` and the second half fails.
    #[test]
    fn the_report_follows_the_file_and_clears_when_it_is_fixed() {
        let owner = Owner::new();
        owner.run(|| {
            let (config, keys) = Config::seeded("");
            assert_eq!(keys.report(), None, "a usable config reports nothing");
            config.write("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
            assert!(
                keys.report()
                    .is_some_and(|report| report.contains("kill-server")),
                "a broken save is reported without any keystroke",
            );
            config.write("[[bind]]\nkey = \"x\"\naction = \"detach-client\"\n");
            assert_eq!(keys.report(), None, "and the fix clears it");
        });
    }

    /// **Every key this client's own chords name must be one a user could BIND**, or the keymap has a
    /// key in it that no `[[bind]]` line can reach and no `list-keys` output can explain.
    ///
    /// A third drift guard in the family R235 started (the wire's encoder and `sprag-tui`'s decoder
    /// are the other two), and the one this frontend needs: pinion's key names are pinion's, so the
    /// only names sprag can hold itself to are the ones it spells.
    #[test]
    fn every_key_the_gui_chords_name_is_bindable() {
        for key in [
            "PageUp",
            "PageDown",
            "Enter",
            "ArrowUp",
            "ArrowDown",
            "c",
            "v",
            "f",
            "p",
            "l",
        ] {
            assert!(
                sprag_input::is_key_name(key),
                "{key:?} is a key this client reserves but no config could name",
            );
        }
    }

    /// End-to-end multi-pane routing THROUGH `apply_key`: a focused keystroke reaches
    /// ONLY the focused pane's PTY (route_key resolves the focus tag -> pane index ->
    /// `host.send_key(active)`), and a sibling pane receives nothing. A seeded 2-pane
    /// `cat` terminal makes the echo deterministic. This is the full public path
    /// (apply_key -> route_key -> client send), restoring the pre-R110 net that R110's
    /// direct `host.send_key` + negative-only gate tests had dropped (session review).
    #[test]
    fn apply_key_routes_to_the_focused_pane_only() {
        let host = Host::new((40, 6));
        let id0 = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        let id1 = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        let h0 = host.pane_handle(id0).expect("pane 0 handle");
        let h1 = host.pane_handle(id1).expect("pane 1 handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host); // use_terminal() now returns these two cat panes
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // Focus pane 1: each key routes to pane 1's PTY only.
            for ch in ["h", "i"] {
                assert!(TerminalViewer::apply_key(
                    &mut scene,
                    Some(pane_tag(1)),
                    ch,
                    Modifiers::default()
                ));
            }
        });
        assert!(
            wait_for_row0(&h1, "hi").contains("hi"),
            "the focused pane echoes the keys"
        );
        assert!(
            !h0.with_screen(|s| s.row_text(0)).contains("hi"),
            "the unfocused pane received nothing",
        );
    }

    /// `route_key`'s focus gate (via `apply_key`): a non-pane focus (the cosmetic
    /// root tag) and no focus are no-ops that fall through to the shell default —
    /// without resolving a pane or touching a PTY. The inject leg is
    /// [`send_key_routes_to_the_named_pane_only`].
    #[test]
    fn apply_key_gates_on_a_pane_focus() {
        let owner = Owner::new();
        owner.run(|| {
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(!TerminalViewer::apply_key(
                &mut scene,
                Some("sprag_gui"),
                "x",
                Modifiers::default()
            ));
            assert!(!TerminalViewer::apply_key(
                &mut scene,
                None,
                "x",
                Modifiers::default()
            ));
        });
    }

    /// While the client is asking whether to destroy something, a key belongs to the PROMPT even
    /// though a PANE holds focus — the case that matters, since every other route here is focus-gated
    /// and this one must not be.
    ///
    /// Driven through the public `apply_key` so the ROUTER's gate is what is under test, not
    /// [`confirm::handle_key`](crate::confirm::handle_key) called directly: the arrow moving the
    /// prompt's choice and the `Enter` performing the kill are only reachable if the gate is where it
    /// claims to be, and if it were missing both keys would be encoded to the focused pane's PTY
    /// instead.
    ///
    /// REVERT-PROOF: delete the `confirm::handle_key` gate from [`route_key`] and this fails — the
    /// choice never moves and no window is killed. (Measured: without this test that deletion left the
    /// whole suite green, which is why the test exists.)
    #[test]
    fn a_key_cannot_reach_a_pane_while_a_destructive_prompt_is_up() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();
            crate::confirm::run_or_arm(
                crate::command::Command::KillWindow(victim),
                None,
                &terminal.slots,
            );
            assert!(crate::confirm::is_open(), "a prompt is up");

            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // A pane holds focus throughout: these keys would otherwise be PTY bytes.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "ArrowRight",
                Modifiers::default()
            ));
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                Modifiers::default()
            ));

            assert!(
                !crate::confirm::is_open(),
                "the prompt was answered through the router, not the pane"
            );
            assert_eq!(
                use_terminal().slots.windows().len(),
                before - 1,
                "and the answer is what performed the kill"
            );
        });
    }

    /// The composition overlay + focus gate (PTY-free): `Update` mirrors into the
    /// focused pane's [`use_preedit`] overlay WITHOUT touching any PTY, a non-pane
    /// composition is a no-op, and an empty commit clears the overlay (the empty
    /// guard skips `send_text`, so this spawns no session). The committed-text WRITE
    /// is [`send_text_writes_committed_text_to_the_named_pane`]. `apply_composition`
    /// ignores its scene arg now (input is a client send), so a trivial scene suffices.
    ///
    /// Scope (honest): synthetic `CompositionEvent`s exercise sprag's preedit-overlay
    /// seam, NOT the live platform IME's `Start`/`Update`/`Commit` sequencing
    /// (verified separately against ibus-hangul in a live window).
    #[test]
    fn apply_composition_overlays_preedit_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let commit = |t: &str| CompositionEvent::Commit(t.to_owned());
            // Focus gate: a non-pane composition is a no-op.
            assert!(!TerminalViewer::apply_composition(
                &mut scene,
                None,
                &commit("한")
            ));
            // Update mirrors into pane 0's overlay (NOT written to any PTY).
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &CompositionEvent::Update("ㅎ".to_owned())
            ));
            assert_eq!(
                use_preedit(0).get(),
                "ㅎ",
                "the overlay mirrors the in-progress composition"
            );
            // An empty commit clears the overlay and writes nothing.
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &commit("")
            ));
            assert_eq!(
                use_preedit(0).get(),
                "",
                "an empty commit clears the overlay"
            );
        });
    }

    /// End-to-end IME commit THROUGH `apply_composition`: a non-empty Commit on the
    /// focused pane clears the overlay AND writes the literal UTF-8 to that pane's PTY
    /// (route_composition's Commit arm -> `host.send_text`), echoed through the
    /// cooked-mode `cat`. Uses a seeded `cat` terminal for a deterministic echo. This
    /// restores the pre-R110 commit-write net that R110's direct `host.send_text`
    /// (which bypassed route_composition's Commit arm) had dropped (session review).
    #[test]
    fn apply_composition_commit_writes_to_the_focused_pane() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host); // use_terminal() now returns this cat pane
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &CompositionEvent::Commit("한글".to_owned())
            ));
            assert_eq!(use_preedit(0).get(), "", "the commit clears the overlay");
        });
        assert!(
            wait_for_row0(&handle, "한글").contains("한글"),
            "the committed text reached the focused pane's PTY",
        );
    }

    /// The R705.1 contract sprag relies on for live preedit: a composing `set` of
    /// a pane's preedit Signal — which `view` subscribes to by reading it every
    /// frame — flips the root owner dirty, which the shell's reactive-dirty bridge
    /// turns into a repaint. Pins the coupling so a pinion bridge regression fails
    /// HERE rather than silently degrading live IME.
    #[test]
    fn preedit_set_flips_owner_dirty_for_repaint() {
        let owner = Owner::new();
        // Mirror `view`: subscribe the owner by reading the preedit Signal.
        owner.run(|| {
            let _ = use_preedit(0).get();
        });
        owner.clear_dirty();
        assert!(!owner.is_dirty(), "clean after a subscribing read");
        // A composing Update sets the subscribed Signal.
        owner.run(|| use_preedit(0).set("ㅎ".to_owned()));
        assert!(
            owner.is_dirty(),
            "a preedit set flips the owner dirty (R705.1 repaint arm)"
        );
    }

    #[test]
    fn page_delta_signs_match_scroll_direction() {
        // PageUp walks toward older history -> DECREASE offset_y (top-anchored).
        assert_eq!(page_delta("PageUp", 13), -13);
        // PageDown walks back toward the live bottom -> INCREASE offset_y.
        assert_eq!(page_delta("PageDown", 13), 13);
        // A non-page key does not scroll.
        assert_eq!(page_delta("a", 13), 0);
    }

    #[test]
    fn window_chord_recognizes_the_reserved_chords() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(window_chord("PageUp", ctrl), Some(WindowChord::CycleFocus));
        assert_eq!(
            window_chord("PageDown", ctrl),
            Some(WindowChord::CycleFocus)
        );
        assert_eq!(window_chord("PageUp", shift), Some(WindowChord::Scroll));
        assert_eq!(
            window_chord("Enter", ctrl_shift),
            Some(WindowChord::ToggleDock)
        );
        // Ctrl+Shift+Arrow jumps between prompts (jump-to-prompt); it shares the reserved
        // Ctrl+Shift space with the dock/clipboard/session chords but takes the arrows.
        assert_eq!(
            window_chord("ArrowUp", ctrl_shift),
            Some(WindowChord::JumpPrompt)
        );
        assert_eq!(
            window_chord("ArrowDown", ctrl_shift),
            Some(WindowChord::JumpPrompt)
        );
        // An arrow WITHOUT both modifiers injects (Shift+Arrow selects, Ctrl+Arrow word-moves).
        assert_eq!(window_chord("ArrowUp", ctrl), None);
        assert_eq!(window_chord("ArrowUp", shift), None);
        // Ctrl+Shift+Page is NOT a window chord — it is a ROOT BINDING (`switch-client`, R314);
        // the page chords here take EXACTLY ONE of Ctrl / Shift, so neither shadows the other.
        assert_eq!(window_chord("PageUp", ctrl_shift), None);
        assert_eq!(window_chord("PageDown", ctrl_shift), None);
        // A normal keystroke is not a chord (it injects).
        assert_eq!(window_chord("a", Modifiers::default()), None);
        assert_eq!(window_chord("Enter", Modifiers::default()), None);
        assert_eq!(window_chord("PageUp", Modifiers::default()), None);
    }

    /// The pure jump-to-prompt math: `ArrowUp` finds the nearest prompt above the view top,
    /// `ArrowDown` the nearest below, each clamped to `[0, max]` (a prompt still in the
    /// visible grid resolves to the live bottom), `None` when there is none that way.
    #[test]
    fn jump_target_walks_prompts_from_the_view_top() {
        // Prompts at logical lines 2, 5, 9; scrollable bound (scrollback_len) = 7, so the
        // prompt at 9 is in the visible grid and resolves to the live bottom (7).
        let positions = [2usize, 5, 9];
        let max = 7;
        // From the live bottom (7): up jumps to 5 (nearest above), down finds nothing new.
        assert_eq!(jump_target(&positions, 7, max, "ArrowUp"), Some(5));
        assert_eq!(jump_target(&positions, 7, max, "ArrowDown"), None);
        // From line 5: up jumps to 2, down jumps to 9-clamped-to-7 (the live view).
        assert_eq!(jump_target(&positions, 5, max, "ArrowUp"), Some(2));
        assert_eq!(jump_target(&positions, 5, max, "ArrowDown"), Some(7));
        // From the oldest (0): nothing above; down jumps to the first prompt (2).
        assert_eq!(jump_target(&positions, 0, max, "ArrowUp"), None);
        assert_eq!(jump_target(&positions, 0, max, "ArrowDown"), Some(2));
        // No prompts, or a non-arrow key: a no-op.
        assert_eq!(jump_target(&[], 3, max, "ArrowUp"), None);
        assert_eq!(jump_target(&positions, 5, max, "Enter"), None);
    }

    /// The session PAGE chords are BINDINGS now (R314), not a private table — so this drives the
    /// shared keymap and reads back the ACTION, which is the thing a user can also see in
    /// `sprag list-keys`.
    ///
    /// ⚠ It used to call a `session_chord` function in this file that R314 deleted, and RE-AIMING
    /// it found a live defect in the replacement: the third chord, `Ctrl+Shift+L`, was bound as
    /// `C-S-L` and **that binding also took `Ctrl-L`** — the shell's clear-screen — because
    /// `KeySpec::matches` masks Shift off a character key (R306) and folds an ASCII letter's case
    /// under `Ctrl`. It is unbound now and `prefix L` carries the verb; the last two assertions
    /// here are what say so, and they are the reason this test exists in this form.
    #[test]
    fn the_session_chords_are_root_bindings_of_the_shared_vocabulary() {
        use sprag_host::keymap::{Keymap, PrefixMode, Routed, SwitchClientAsk};
        use sprag_terminal::OrderStep;
        let keymap = Keymap::default();
        let now = std::time::Instant::now();
        let acted = |key: &str, mods: sprag_input::Modifiers| match keymap.route(
            PrefixMode::ToPane,
            now,
            key,
            mods,
        ) {
            Routed::Act { action, .. } => Some(action),
            _ => None,
        };
        let ctrl_shift = sprag_input::Modifiers {
            ctrl: true,
            shift: true,
            ..sprag_input::Modifiers::default()
        };
        let switching = |ask| Some(BoundAction::SwitchClient { ask });
        assert_eq!(
            acted("PageDown", ctrl_shift),
            switching(SwitchClientAsk::Step(OrderStep::Next)),
        );
        assert_eq!(
            acted("PageUp", ctrl_shift),
            switching(SwitchClientAsk::Step(OrderStep::Previous)),
        );
        // Disjoint from the OTHER Ctrl+Shift chords, which this vocabulary must NOT have taken:
        // clipboard copy/paste and the dock toggle are still the GUI's own and still reach it,
        // because an unbound key routes `ToPane` and falls through to the chord checks.
        assert_eq!(acted("c", ctrl_shift), None);
        assert_eq!(acted("v", ctrl_shift), None);
        assert_eq!(acted("Enter", ctrl_shift), None);
        // Both modifiers are required, and Alt excludes it — for a NAMED key `KeySpec::matches`
        // compares Shift exactly, so this is a property of the binding rather than of a
        // hand-written predicate.
        for mods in [
            sprag_input::Modifiers {
                ctrl: true,
                ..sprag_input::Modifiers::default()
            },
            sprag_input::Modifiers {
                shift: true,
                ..sprag_input::Modifiers::default()
            },
            sprag_input::Modifiers {
                ctrl: true,
                shift: true,
                alt: true,
                ..sprag_input::Modifiers::default()
            },
        ] {
            assert_eq!(
                acted("PageUp", mods),
                None,
                "{mods:?} is not the chord, so the page key stays the GUI's own",
            );
        }
        // ⚠ `Ctrl+Shift+L` IS NOT BOUND, and `Ctrl-L` STILL REACHES THE SHELL. A `C-S-L` binding
        // takes both, because Shift is masked off a character key and a letter's case is folded
        // under Ctrl — so the first assertion is the feature that was deliberately dropped and the
        // second is the clear-screen it would have cost every pane in every client.
        for mods in [
            ctrl_shift,
            sprag_input::Modifiers {
                ctrl: true,
                ..sprag_input::Modifiers::default()
            },
        ] {
            for key in ["L", "l"] {
                assert_eq!(
                    acted(key, mods),
                    None,
                    "{key} under {mods:?} belongs to the program in the pane",
                );
            }
        }
        // THE CONTROL: the page keys with no modifiers are the pane's too, so the assertions above
        // are about the chord and not about the key.
        for key in ["PageUp", "PageDown"] {
            assert_eq!(
                acted(key, sprag_input::Modifiers::default()),
                None,
                "{key} unmodified belongs to the program in the pane",
            );
        }
    }

    /// Session cycling is the wrapping LIST neighbour of the current session — `forward` to the next,
    /// else the previous; a single-session list yields the current one (a `switch_session` no-op),
    /// and an unknown current yields `None`.
    #[test]
    fn session_neighbour_wraps_the_list_both_ways() {
        let names = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(session_neighbour(&names, "a", true).as_deref(), Some("b"));
        assert_eq!(session_neighbour(&names, "c", true).as_deref(), Some("a")); // wrap forward
        assert_eq!(session_neighbour(&names, "a", false).as_deref(), Some("c")); // wrap backward
        assert_eq!(session_neighbour(&names, "b", false).as_deref(), Some("a"));
        // Alone: the neighbour is the current session, which switch_session no-ops.
        let solo = vec!["only".to_owned()];
        assert_eq!(
            session_neighbour(&solo, "only", true).as_deref(),
            Some("only")
        );
        // Current not in the list: nothing to anchor on.
        assert_eq!(session_neighbour(&names, "gone", true), None);
    }

    /// Forward = next, backward = previous; both wrap over the OCCUPIED slots.
    ///
    /// The direction is the caller's now (a `bool`), not read off a key here — which key means
    /// forward is [`window_chord`]'s answer and is asserted there, and the keymap's
    /// `select-pane -t :.+` has no key to read at all.
    #[test]
    fn next_focus_wraps_between_panes() {
        // Contiguous (the boot set) behaves as the former modular wrap.
        let all = [0, 1, 2];
        assert_eq!(next_focus(0, true, &all), Some(1));
        assert_eq!(next_focus(2, true, &all), Some(0)); // wrap forward
        assert_eq!(next_focus(0, false, &all), Some(2)); // wrap backward
        assert_eq!(next_focus(1, false, &all), Some(0));
        // A single pane has nowhere to switch to.
        assert_eq!(next_focus(0, true, &[0]), None);
        // Non-contiguous slots (a closed pane left a hole at slot 1): cycling STEPS
        // OVER the hole rather than landing on it (the Round 2b live-correct path).
        let holed = [0, 2, 3];
        assert_eq!(next_focus(0, true, &holed), Some(2));
        assert_eq!(next_focus(3, true, &holed), Some(0)); // wrap forward
        assert_eq!(next_focus(0, false, &holed), Some(3)); // wrap backward
        // `active` not among the occupied slots (a just-closed slot) -> nowhere.
        assert_eq!(next_focus(1, true, &holed), None);
    }

    /// The `Ctrl+PageUp/Down` focus-cycle chord is handled (`true`), reaches no
    /// PTY, and fires a pinion [`focus_request`](pinion_core::focus_request) for
    /// the correct neighbour pane — the end-to-end focus-switch wiring
    /// (`route_key` -> `cycle_focus` -> `focus_request`), short of the winit event
    /// loop draining it (pinion's own, proven). The Ctrl branch returns before
    /// touching the scene's External, so a minimal model scene suffices;
    /// `cycle_focus` reads `use_terminal()`'s pane count (the 2 boot panes).
    #[test]
    fn ctrl_page_chord_cycles_focus_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            let _ = pinion_core::focus_request::drain(); // clear any stale request
            // Ctrl+PageDown from pane 0 requests focus on the next pane.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageDown",
                ctrl
            ));
            assert_eq!(
                pinion_core::focus_request::drain().as_deref(),
                Some(pane_tag(1)),
                "Ctrl+PageDown cycles focus to the next pane",
            );
            // Ctrl+PageUp from pane 0 wraps backward to the last pane (pane 1 of 2).
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageUp",
                ctrl
            ));
            assert_eq!(
                pinion_core::focus_request::drain().as_deref(),
                Some(pane_tag(1)),
                "Ctrl+PageUp wraps focus backward",
            );
        });
    }

    /// `Ctrl+Shift+Enter` toggles the focused pane's dock state — handled
    /// (`true`), reaches no PTY (the branch returns before the External invoke),
    /// and round-trips the dock topology Signal (undock adds a window, dock-back
    /// removes it). The window-management sibling of the focus-cycle chord.
    #[test]
    fn ctrl_shift_enter_toggles_dock_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let windows = crate::dock::use_windows_topology();
            assert_eq!(windows.get().len(), 1, "starts with the main window only");
            let ctrl_shift = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            // Undock the focused pane (handled, not to the PTY).
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift
            ));
            assert_eq!(
                windows.get().len(),
                2,
                "the focused pane undocked into its own window"
            );
            // The same chord docks it back.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift
            ));
            assert_eq!(windows.get().len(), 1, "docked back");
        });
    }

    /// Auto-repeat of the DISCRETE dock chord is dropped (consumes pinion R1071 /
    /// PINION-PR27): the live shell drives `apply_key_repeat` with the platform
    /// `KeyEvent.repeat` flag, and a held `Ctrl+Shift+Enter` must toggle ONCE per
    /// press, not on every OS auto-repeat — that re-send is what made it
    /// "dock-then-undock" in the multi-window state. The leading press
    /// (`repeat == false`) toggles; an auto-repeat (`repeat == true`) is a no-op. A
    /// scrollback chord is CONTINUOUS, so it is NOT dropped (a held `Shift+PageUp`
    /// keeps scrolling) — `route_key` drops only the discrete chords.
    #[test]
    fn dock_chord_auto_repeat_is_dropped_scroll_repeats() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let windows = crate::dock::use_windows_topology();
            let ctrl_shift = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };

            // An OS auto-repeat re-send of the dock chord is dropped: no-op, no window.
            assert!(
                !TerminalViewer::apply_key_repeat(
                    &mut scene,
                    Some(pane_tag(0)),
                    "Enter",
                    ctrl_shift,
                    true
                ),
                "a held dock chord's auto-repeat is a no-op",
            );
            assert_eq!(windows.get().len(), 1, "auto-repeat did not undock");

            // The leading press (repeat = false) toggles once.
            assert!(TerminalViewer::apply_key_repeat(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift,
                false
            ));
            assert_eq!(windows.get().len(), 2, "the leading press undocked once");

            // A scrollback chord is continuous — a held Shift+PageUp keeps scrolling
            // (NOT dropped), so only the discrete chords are repeat-gated.
            assert!(
                TerminalViewer::apply_key_repeat(
                    &mut scene,
                    Some(pane_tag(0)),
                    "PageUp",
                    shift,
                    true
                ),
                "scrollback repeats on a held key",
            );
        });
    }

    #[test]
    fn scroll_state_is_per_pane_and_defaults_live() {
        use crate::scrollbar::use_pane_scroll;
        let owner = Owner::new();
        // Each pane boots at the live bottom (offset_y 0 == max 0) and scrolls
        // independently.
        assert_eq!(
            owner.run(|| use_pane_scroll(0).offset_y()),
            0,
            "pane 0 boots live"
        );
        assert_eq!(
            owner.run(|| use_pane_scroll(1).offset_y()),
            0,
            "pane 1 boots live"
        );
        owner.run(|| {
            let s = use_pane_scroll(1);
            s.set_max(0, 20);
            s.scroll_to(0, 7);
        });
        assert_eq!(owner.run(|| use_pane_scroll(1).offset_y()), 7);
        assert_eq!(
            owner.run(|| use_pane_scroll(0).offset_y()),
            0,
            "pane 0 is unaffected (per-pane slots)"
        );
    }

    /// R122 (Round 2b): `reset_pane_preedit` clears a slot's in-progress composition when
    /// the slot frees, so a slot reused by a later pane shows no inherited preedit overlay.
    #[test]
    fn reset_pane_preedit_clears_the_composition() {
        Owner::new().run(|| {
            let preedit = use_preedit(2);
            preedit.set("がん".to_owned());
            assert_eq!(preedit.get(), "がん");
            reset_pane_preedit(2);
            assert_eq!(preedit.get(), "", "the freed slot's preedit is cleared");
        });
    }

    /// `apply_key` treats Shift+PageUp as a scroll (handled, not sent to the PTY)
    /// of the focused pane and snaps that pane's view back to the live bottom
    /// (offset_y == max) on any other (typed) key.
    #[test]
    fn apply_key_scrolls_and_snaps_to_bottom() {
        use crate::scrollbar::use_pane_scroll;
        let owner = Owner::new();
        owner.run(|| {
            // The model scene over the live boot pane 0 (same pane use_terminal
            // caches), so scroll_view reads the pane the routing addresses.
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // Shift+PageUp is consumed as a scroll (true = handled, not the PTY).
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageUp",
                shift
            ));
            // Pause partway up history (offset_y 3 of a 10-row depth), then a typed
            // key snaps to the live bottom (offset_y == max).
            let scroll = use_pane_scroll(0);
            scroll.set_max(0, 10);
            scroll.scroll_to(0, 3);
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "a",
                Modifiers::default()
            ));
            assert_eq!(
                scroll.offset_y(),
                scroll.max().1,
                "typing snaps to the live bottom"
            );
            assert_eq!(
                scroll.offset_y(),
                10,
                "the live bottom is the reconciled max"
            );
        });
    }
}
