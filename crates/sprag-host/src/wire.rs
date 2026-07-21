//! The wire ABI vocabulary — the ONE definition of the JSON-RPC address grammar
//! and action names the host serves and a client addresses.
//!
//! pinion's `scene/invoke` / `scene/query` address a node as
//! `/{container_tag}/{external_tag}/external/{action}`; sprag owns the tags
//! ([`INPUT_TAG`] / [`MUX_TAG`]) and the action
//! names. Before this module those were transcribed in three places — the host's
//! own externals ([`SpragPaneExternal`](crate::SpragPaneExternal) /
//! [`WorkspaceExternal`](crate::WorkspaceExternal) schema + dispatch), the host's
//! tests, and the wire client (`sprag-gui`'s `WireHost`) — so a path or action
//! rename could silently desync a client. This module is the single home: the host
//! externals match on these consts, and the client builds its request paths from the
//! same builders, so the ABI is defined once.

use pinion_core::external::{SchemaArg, SchemaField};

use crate::{INPUT_TAG, MUX_TAG};

/// The pane-input external invoke action that injects a key (W3C key + mods →
/// PTY bytes, the R2.6 encoder).
pub const KEY_ACTION: &str = "key";
/// The pane-input external invoke action that writes literal UTF-8 (IME commit /
/// paste), no key-encoding.
pub const TEXT_ACTION: &str = "text";
/// The pane-input external invoke action that PASTES literal UTF-8: like [`TEXT_ACTION`], but the
/// host wraps it in the bracketed-paste markers (`ESC [ 200 ~` … `ESC [ 201 ~`) when the pane's
/// child has enabled DEC private mode 2004, and filters any embedded end marker so the paste
/// cannot break out of the bracket. Distinct from [`TEXT_ACTION`] because only a paste is
/// bracketed — typed / IME-committed text never is. The bracketing decision lives at the PTY
/// boundary (which holds the authoritative mode), so a display client just forwards the raw text.
pub const PASTE_ACTION: &str = "paste";
/// The pane-input external query slot: how many distinct frames [`CELLS_FIELD`] can address
/// — `scrollback_len + 1` (the live view, plus one per retained history line).
///
/// It exists to make [`CELLS_FIELD`]'s argument able to state its bound. A count, not a
/// length, and the
/// `+ 1` is the whole point: offsets run `0..=scrollback_len` INCLUSIVE (`0` is live,
/// `scrollback_len` is fully scrolled back, both answerable), so the EXCLUSIVE `0..frames`
/// that [`ArgDomain::IndexOf`](pinion_core::external::ArgDomain::IndexOf) means is exactly
/// right against `scrollback_len + 1` and would have been false against the length.
pub const FRAMES_SLOT: &str = "frames";

/// The arguments of [`CELLS_FIELD`] — one scrollback `offset`, bounded by [`FRAMES_SLOT`].
///
/// **`IndexOf`, not `Open`, and R155's review is why.** The first draft declared
/// [`ArgDomain::Open`](pinion_core::external::ArgDomain::Open) with a confident paragraph
/// arguing the bound was "real but not expressible", citing pinion's `datepicker` as the
/// same shape. **pinion's datepicker comment refutes that analogy in its own words**: its
/// `state.<day>` is inexpressible because it is ONE-BASED (`1..=days`, so `IndexOf` "would be
/// false at BOTH ends"), and it *publishes* `days` regardless. This offset is ZERO-based, so
/// `0..=scrollback_len` is plainly `0..(scrollback_len + 1)` — an `IndexOf` against a count
/// this surface simply had to publish. `Open` was not a bound we could not state; it was a
/// count we had not exposed, and pinion's own doc calls an unearned `Open` "an affirmative
/// false statement … worse than the pre-R1353 silence it replaced, because now it carries a
/// schema's authority".
///
/// An out-of-range offset still CLAMPS rather than erroring (`project_scrolled`), which
/// `IndexOf` does not promise away — pinion declares `width.<col>` the same way over a
/// clamping reader. The domain says which offsets are MEANINGFUL, so an agent reads one
/// scalar instead of fetching whole cell grids to discover where history ends.
const CELLS_ARGS: &[SchemaArg] = &[SchemaArg::index("offset", FRAMES_SLOT)];

/// The pane-input external query FAMILY: the pane's cell FRAME
/// ([`CellFrame`](crate::CellFrame)) at scrollback `offset` — `cells.<offset>`, where
/// `cells.0` is the live view a display client projects each frame and a larger offset
/// windows into history.
///
/// **This declaration is the ONE definition of the family**, and everything else derives
/// from it rather than re-spelling it: the host's schema publishes it, the host's `query`
/// strips [`SchemaField::literal_prefix`] off an arriving path, and both ends build an
/// address through [`cells_slot_at`]. So the template an agent discovers in `$schema`, the
/// prefix the host matches, and the path a client sends cannot drift apart —
/// `the_cells_family_declares_the_paths_it_answers` holds them to it.
///
/// ## Why ONE name, and why a query
///
/// This was TWO wire names until PR-61 landed (a `frame` query for the live view and a
/// `cells` invoke for history), and the reason was a mistake worth keeping recorded. A
/// display client re-reads the live frame on every scene-revision move, an invoke is a
/// `MethodOcc::Mutate` that bumps the revision, so serving the live frame through an action
/// woke the very waiter it answered — a ~30Hz idle livelock that burned a full core (R152).
/// The livelock was real and the query fixed it; the SPLIT was the part that was wrong.
/// sprag split the concept because it read `query(&self, path: &str)`'s argument-free
/// signature as "a parameterized read is impossible" — and PR-61's answer (pinion R1352) was
/// that it never was: **the argument rides the path**, as `width.<col>` and `id_at.<pos>`
/// already did upstream. What was genuinely missing was the ability to SAY so, which R1353
/// then delivered ([`SchemaField::parametric`]) — so the family is now discoverable rather
/// than conventional.
///
/// Collapsing the names is what retires the split's last defect. A scrollback read was still
/// an invoke, so it still bumped: one client's wheel tick woke every OTHER attached client's
/// parked `waitFor` into a full re-fetch. Bounded and terminating, so never the livelock —
/// but the same defect, and one door to one concept is what removes it.
pub const CELLS_FIELD: SchemaField = SchemaField::parametric("cells.<offset>", "frame", CELLS_ARGS);
/// The pane-input external query slot: the pane's full output text (scrollback +
/// visible).
pub const FULL_TEXT_SLOT: &str = "full_text";
/// The pane-input external query slot: the producer's DECCKM (application cursor
/// keys) mode.
pub const CURSOR_KEYS_SLOT: &str = "application_cursor_keys";
/// The pane-input external query slot: the LAST shell command sliced from the pane's OSC 133
/// marks — a JSON object `{command, output, exit_status, running}`
/// ([`Screen::last_command`](sprag_vt::Screen::last_command)), or `null` when no command has run
/// under shell integration (the agent then falls back to [`FULL_TEXT_SLOT`]). The command-scoped
/// read tmux's whole-pane `capture-pane` cannot express.
pub const LAST_COMMAND_SLOT: &str = "last_command";
/// The pane-input external query slot: the OSC 133 prompt-mark positions
/// ([`Screen::prompt_positions`](sprag_vt::Screen::prompt_positions)) — a JSON array of logical
/// line indices (from the oldest retained line, the scroll `offset_y` unit) a display client's
/// jump-to-prompt scrolls to. Read ON DEMAND (a keyboard jump), never per frame.
pub const PROMPT_MARKS_SLOT: &str = "prompt_marks";
/// The pane-input external query slot: the OSC-8 hyperlink runs on the visible grid
/// ([`Screen::hyperlink_runs`](sprag_vt::Screen::hyperlink_runs)) — a JSON array of
/// `{text, uri, id}`, one per contiguous link run, or `[]` when the pane shows no links. An agent
/// reads a link's DESTINATION as data (the URI, without OCR), which tmux's `capture-pane` cannot
/// expose because it flattens OSC 8 to plain text. Read ON DEMAND (a `read_pane_links` call).
pub const LINKS_SLOT: &str = "links";
/// The pane-input external query slot: the pane's most recent OSC 52 clipboard WRITE — a JSON
/// object `{targets:{clipboard,primary}, text, seq}`, or `null` when the child has written none.
/// Fetched ON DEMAND when the `clipboard_write_seq` in the pane list grows (the payload can be a
/// whole paste, so it is not carried per poll). A client applies it — subject to its clipboard
/// policy — to its own system clipboard.
pub const CLIPBOARD_WRITE_SLOT: &str = "clipboard_write";
/// The pane-input external action: ANSWER a pending OSC 52 read query. Params `{seq, sel, text}`
/// — the query `seq` a display client saw in the pane list, the selection char (`c`/`p`) it read,
/// and that selection's current text. The host writes the `OSC 52` reply back to the PTY, admitting
/// EXACTLY ONE reply per query across all attached clients; the answer reports `{wrote}`.
pub const CLIPBOARD_ANSWER_ACTION: &str = "clipboard_answer";

/// The arguments of [`IMAGE_DATA_FIELD`] — one inline image `id`, `Open`. Unlike
/// [`CELLS_FIELD`]'s bounded `IndexOf`, a Kitty image id is a PRODUCER-CHOSEN key, not sprag's
/// index into a count, so there is no count to publish; the ids are the open set the child
/// transmitted (enumerated in the panes-slot `images` summary). `Open` is earned here, not a
/// default hiding a count.
const IMAGE_DATA_ARGS: &[SchemaArg] = &[SchemaArg::open("id", "int")];
/// The pane-input external query FAMILY: one inline image's RGBA as base64 (`image_data.<id>`,
/// R1404 Stage 5). Fetched ON DEMAND when a display client sees a NEW / CHANGED image in the
/// panes-slot `images` summary (keyed on `{id, seq}`) — the RGBA is up to
/// [`MAX_IMAGE_BYTES`](sprag_vt) and must NOT ride the per-poll panes slot, the
/// [`FULL_TEXT_SLOT`] / [`CLIPBOARD_WRITE_SLOT`] on-demand precedent. `Null` for an id the pane
/// is not currently showing.
pub const IMAGE_DATA_FIELD: SchemaField =
    SchemaField::parametric("image_data.<id>", "string", IMAGE_DATA_ARGS);

/// The pane-input external's DECLARED SCHEMA — every path it answers, with its type and any
/// arguments, in `$schema` order. [`SpragPaneExternal`](crate::SpragPaneExternal) publishes
/// this verbatim.
///
/// The declarations live HERE, beside the addresses, because this module claims to be "the
/// ONE definition of the … grammar and action names" and a field's TYPE is part of its
/// declaration. R155's review caught the module owning `CELLS_FIELD` whole while the other
/// four fields kept their names here and their types at the use site — one concept, two
/// homes, split by nothing more principled than which field happened to be parametric.
///
/// Each entry reuses its address const rather than re-spelling it, so a field's path and the
/// path a client builds are the same string by construction.
pub const PANE_SCHEMA: &[SchemaField] = &[
    SchemaField::new(KEY_ACTION, "action"),
    SchemaField::new(TEXT_ACTION, "action"),
    SchemaField::new(PASTE_ACTION, "action"),
    CELLS_FIELD,
    SchemaField::new(FRAMES_SLOT, "int"),
    SchemaField::new(CURSOR_KEYS_SLOT, "bool"),
    SchemaField::new(FULL_TEXT_SLOT, "string"),
    SchemaField::new(LAST_COMMAND_SLOT, "object"),
    SchemaField::new(PROMPT_MARKS_SLOT, "array"),
    SchemaField::new(LINKS_SLOT, "array"),
    IMAGE_DATA_FIELD,
    SchemaField::new(CLIPBOARD_WRITE_SLOT, "object"),
    SchemaField::new(CLIPBOARD_ANSWER_ACTION, "action"),
];

/// The out-of-band request param naming the SESSION a request acts on — `{"session": "work"}`
/// alongside `path` / `args`, never part of the address.
///
/// ## Why a param and not connection identity
///
/// One daemon holds every session, so a request must say which one it is about. The obvious
/// shape — "the host learns who is asking from the connection" — is not available and should
/// not be: pinion's `RpcFrame` carries a request string and a reply sink and no identity, so
/// teaching the funnel about connections would be an upstream ask. It is also the wrong
/// model. Which session a client is attached to is the CLIENT's state; it knows it, and
/// sending it makes each request self-describing and the host free of per-connection
/// bookkeeping. tmux's `switch-client` then costs nothing here — a client switches by sending
/// a different name.
///
/// ## The contract, copied from pinion's own precedent
///
/// pinion carries the same shape for its display windows (`Request::window_scope`, R890.1
/// §5.49 — a different concept that merely shares the word "window"), and sprag mirrors it
/// deliberately rather than inventing a second convention for one idea:
///
/// * **absent** → the default scope ([`SessionRegistry::default_session`](sprag_terminal::SessionRegistry::default_session));
/// * **a string** → the session of that name;
/// * **present but not a string** → `-32602`, before method routing, never a silent fallback
///   to the default. pinion's doc records why in its own scars: silently dropping a
///   malformed scope made the request act on the primary — "wrong target for writes, wrong
///   data for reads" — and that survived an entire campaign to kill aliasing precisely
///   because it hid in the type-error corner;
/// * **a name no session carries** → refused wholesale, for the same reason.
///
/// The string itself is defined by the transport client that WRITES it
/// ([`sprag_rpc::SESSION_PARAM`]) and re-exported here for the host that READS it, so the two
/// ends of the wire share ONE spelling. This doc is the reader's contract; the writer's is on
/// the definition.
pub use sprag_rpc::SESSION_PARAM;

/// The client-lifecycle wire vocabulary (R-PR67 Stage 1), re-exported from the transport client
/// that WRITES it ([`sprag_rpc`]) so the host that READS it shares ONE spelling — exactly as
/// [`SESSION_PARAM`] is. [`CLIENT_HELLO_METHOD`] announces a connection's client id
/// ([`CLIENT_PARAM`]); [`CLIENT_ATTACH_METHOD`] declares/switches that client's attached session
/// (reusing [`SESSION_PARAM`]). Both are intercepted before the generic dispatch core, since they
/// act on the frame's connection id, which no scene external sees. The reader's contract lives in
/// [`crate::rpc`] (the dispatch owner's client-lifecycle intercept); the writer's is on each
/// `sprag_rpc` const.
pub use sprag_rpc::{CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM};

/// The mux control external query slot: every session's name, plus which one an unscoped
/// request acts on — how a client discovers what it can address with [`SESSION_PARAM`].
///
/// Served rather than left to convention: a scope param an agent cannot enumerate is one it
/// must guess at, and the whole point of the scene-as-data surface is that a peer ASKS.
pub const SESSIONS_SLOT: &str = "sessions";

/// The mux control external query slot: every currently-ATTACHED client and the session it is
/// viewing (`[{client, session}]`) — tmux `list-clients`, behind the `sprag list-clients` CLI.
///
/// Registry-WIDE like [`SESSIONS_SLOT`] (its subject is the set of clients, not any one session),
/// and filled HOST-side from the dispatch layer's [`crate::AttachmentRegistry`] — the same
/// per-client state that fills each [`SESSIONS_SLOT`] row's `attached` count. Empty off a daemon
/// that tracks no wire clients (a GUI's in-process host), so it degrades to "no clients".
pub const CLIENTS_SLOT: &str = "clients";

/// The mux control external invoke action that creates a session BORN WITH A SHELL
/// (`{name?, cmd?, cols?, rows?}`), returning its name.
///
/// Creating one spawns its first pane, mirroring tmux's `new-session -x -y [command]`
/// (`cmd`/`cols`/`rows` shape that pane; absent → `$SHELL` at the pool's default size), so on the
/// happy path a session is not empty (only a runtime fork/exec failure leaves it so). A malformed
/// birth spec is rejected before the session is created, the same as a bad `name`. An ACTION, not
/// an `intervene` slot: it is not a plain assignment — the
/// name must be free (a name is an ADDRESS, so a duplicate would make one ambiguous) and the
/// create is refusable. Creating is deliberately NOT attaching: it changes no other client's
/// scope, because nothing can — a `new_session` never moves the default, and every other client
/// names its own.
pub const NEW_SESSION_ACTION: &str = "new_session";

/// The mux control external invoke action that kills a session (`{name}`) — tmux
/// `kill-session`.
///
/// An ACTION, not an `intervene` slot: it removes a session (or, for the last one, drains it
/// and ends the daemon), so it is a lifecycle event a client requests, not an assignment. A
/// non-last kill answers `null`; killing the LAST session ends the server, so its answer may
/// never reach the client (the socket closes as the daemon exits).
pub const KILL_SESSION_ACTION: &str = "kill_session";

/// The mux control external invoke action that spawns a pane, returning its id.
pub const SPAWN_ACTION: &str = "spawn";
/// The mux control external invoke action that closes a pane.
pub const CLOSE_ACTION: &str = "close";
/// The mux control external invoke action that resizes a pane's PTY + emulator.
pub const RESIZE_ACTION: &str = "resize";
/// The mux control external query slot: the live pane list as JSON.
pub const PANES_SLOT: &str = "panes";
/// The mux control external query slot: the LOGICAL layout of the current window of the
/// session the request is SCOPED to ([`SESSION_PARAM`]), plus the
/// revision it is at ([`LayoutSnapshot`](sprag_terminal::LayoutSnapshot)) as JSON — the
/// arrangement a display client projects, and the state that lets a reattaching client
/// restore the user's layout. Logical only: it carries no pixels.
pub const LAYOUT_SLOT: &str = "layout";
/// The mux control external invoke action that INSTALLS a client's settled arrangement
/// (`{tree, expected_revision, expected_window?}`), returning the canonical
/// [`LayoutSnapshot`](sprag_terminal::LayoutSnapshot).
///
/// `expected_revision` is the revision the gesture was authored against — a compare-and-set,
/// so a write against an arrangement that has moved on is REFUSED rather than silently
/// reverting whoever moved it.
///
/// `expected_window` is optional: the NAME of the window the gesture was drawn on. Because the
/// per-window revision has no cross-window ordering, a client that has since switched windows
/// could otherwise land a stale write on a DIFFERENT window whose revision happened to collide;
/// naming the window makes the compare-and-set refuse that too. Absent ⇒ no window check (a
/// single-client / older caller); present but not a string ⇒ malformed, refused like a
/// wrong-typed `expected_revision`.
///
/// The write half of the arc: a client resolves a gesture on its own surface and sends
/// the result here, which is what turns the user's arrangement into session state. It is
/// an ACTION, not an `intervene` slot, because it is not a plain assignment — the host
/// names the dividers the client minted, validates the shape, and answers with the
/// canonical tree ([`LayoutTree::set_from_wire`](sprag_terminal::LayoutTree::set_from_wire)).
pub const SET_LAYOUT_ACTION: &str = "set_layout";
/// The mux control external invoke action that takes a pane OUT of the tiling or puts it
/// back (`{id, floating}`), returning the resulting
/// [`LayoutSnapshot`](sprag_terminal::LayoutSnapshot).
///
/// Float is session state, not a client's private display concern: a floated pane is one
/// the user took out of the tiling, which is the same class of fact as how the rest are
/// split. WHERE the client then puts that pane's window on screen is pixels, and stays the
/// client's alone.
pub const SET_FLOATING_ACTION: &str = "set_floating";

/// The mux control external query slot: the SCOPED session's windows — each window's name and
/// whether it is the current one (`[{name, current}]`) — [`SESSIONS_SLOT`] one level down. How a
/// tabbed client learns which tabs to draw and which is active.
///
/// SCOPED to the request's session (unlike `sessions`, whose subject is the set of sessions):
/// windows are a property OF a session, so this answers about the one the request named.
pub const WINDOWS_SLOT: &str = "windows";

/// The mux control external invoke action that creates a window in the SCOPED session, born with
/// a shell, selects it, and returns its name (`{name?, cmd?, cols?, rows?}`) — tmux `new-window`.
///
/// SCOPED (it acts on the request's session), unlike [`NEW_SESSION_ACTION`] which names a session
/// directly. `name` absent ⇒ the lowest free integer; `cmd`/`cols`/`rows` shape the birth pane,
/// exactly as [`NEW_SESSION_ACTION`]. Selecting the new window is session state — every attached
/// client follows it, as tmux does.
pub const NEW_WINDOW_ACTION: &str = "new_window";

/// The mux control external invoke action that makes a window current in the SCOPED session
/// (`{window}`) — tmux `select-window`. Session state: every attached client follows.
pub const SELECT_WINDOW_ACTION: &str = "select_window";

/// The mux control external invoke action that renames a window of the SCOPED session
/// (`{window?, name}`) — tmux `rename-window`. `window` absent ⇒ the current one; `name` is the
/// new name.
pub const RENAME_WINDOW_ACTION: &str = "rename_window";

/// The mux control external invoke action that kills a window of the SCOPED session (`{window?}`)
/// — tmux `kill-window`. `window` absent ⇒ the current one. Killing the session's LAST window
/// ends the SESSION (and the last session ends the daemon), tmux's "kill the last window ⇒ the
/// session is gone".
pub const KILL_WINDOW_ACTION: &str = "kill_window";

/// The mux control external invoke action that BREAKS a pane out of its window into a new window
/// of the SCOPED session, born current, and returns its name (`{pane, name?}`) — tmux `break-pane`.
///
/// `pane` is the id of the pane to move; its source window is DERIVED (a [`PaneId`](sprag_terminal::PaneId)
/// is registry-unique, so the window that holds it is unambiguous — the caller never names the
/// source). `name` absent ⇒ the lowest free integer window name. Refused (`Rejected`) if the pane's
/// window tiles only that one pane, if an explicit `name` is taken, or if no window holds `pane`.
pub const BREAK_PANE_ACTION: &str = "break_pane";

/// The mux control external invoke action that JOINS a pane into another window of the SCOPED
/// session (`{pane, window}`) — tmux `join-pane`. Answers `{closed_source: bool}`.
///
/// `pane` is the id of the pane to move (its source window is DERIVED); `window` is the DESTINATION
/// window's name. The pane appends as a new tiled leaf; a join that empties the source window
/// closes it (`closed_source: true`). Refused (`Rejected`) if `pane` already lives in `window`, if
/// no window holds `pane`, or if `window` names no window.
pub const JOIN_PANE_ACTION: &str = "join_pane";

/// The container tag of the pane with host id `pane_id` — the `pane_<id>` node the
/// per-pane data grid + input external live under (the head of a pane-addressed
/// wire path). The ONE place this tag is formatted, shared by the scene assembly
/// ([`workspace_scene`](crate::workspace_scene)'s pane container) and
/// [`pane_input_path`].
#[must_use]
pub fn pane_container_tag(pane_id: u64) -> String {
    format!("pane_{pane_id}")
}

/// The `scene/invoke` / `scene/query` path addressing pane `pane_id`'s input
/// external `action` — `/pane_<id>/sprag_input/external/<action>`. Built from the
/// shared tags so the client and the host agree on the grammar by construction.
#[must_use]
pub fn pane_input_path(pane_id: u64, action: &str) -> String {
    format!(
        "/{}/{INPUT_TAG}/external/{action}",
        pane_container_tag(pane_id)
    )
}

/// The `scene/invoke` / `scene/query` path addressing the mux control external's
/// `action` — `/sprag_mux/external/<action>`.
#[must_use]
pub fn mux_action_path(action: &str) -> String {
    format!("/{MUX_TAG}/external/{action}")
}

/// The [`CELLS_FIELD`] query slot addressing the frame at scrollback `offset` —
/// `cells.<offset>` with the argument filled in (`cells_slot_at(0)` is the live view).
///
/// Derived from the declaration's own [`literal_prefix`](SchemaField::literal_prefix)
/// rather than re-spelling `"cells."`, so the address a client sends is built from the
/// same string the schema publishes and the host strips. Compose it with
/// [`pane_input_path`] to address a specific pane.
#[must_use]
pub fn cells_slot_at(offset: usize) -> String {
    format!("{}{offset}", CELLS_FIELD.literal_prefix())
}

#[cfg(test)]
mod tests {
    use pinion_core::external::ArgDomain;

    use super::*;

    #[test]
    fn pane_input_path_matches_the_documented_grammar() {
        assert_eq!(
            pane_input_path(0, KEY_ACTION),
            "/pane_0/sprag_input/external/key"
        );
        assert_eq!(
            pane_input_path(3, &cells_slot_at(12)),
            "/pane_3/sprag_input/external/cells.12"
        );
    }

    /// The declaration says the exact words the wire uses — a TRIPWIRE on the one spelling
    /// everything else derives from.
    ///
    /// **This test's first draft claimed far more and proved less, and R155's review proved
    /// the gap by experiment.** It asserted "the family answers exactly the paths it
    /// advertises — checked with pinion's OWN matcher, the same predicate its dispatch
    /// uses". Three lies in one sentence: `SchemaField::addresses` runs in NO dispatch path
    /// (pinion's `scene/query` calls `intro.query(path).ok_or(UnknownIntrospectPath)`; the
    /// matcher is reachable only through `read_only_or_unknown`, on `intervene`); the test
    /// calls no `query`, so it cannot observe what the family ANSWERS; and the `addresses`
    /// assertions were TAUTOLOGIES — a reviewer re-ran all four against a field renamed to
    /// `frames.<offset>` and every one still passed, because `cells_slot_at` builds its
    /// probe FROM `literal_prefix()`, so both sides move together. It was the R154 scar
    /// ("the test builds its tag from the very const under test") repeated one round later,
    /// wearing a doc that congratulated itself for avoiding it.
    ///
    /// So the tautologies are gone and what remains is the hardcoded spelling the old doc
    /// apologized for ("rather than sprag's spelling of it"). That line is the whole value:
    /// it is the only assertion a rename cannot satisfy, and a rename is the only drift
    /// worth catching here. `the_cells_family_answers_the_paths_it_declares` in `rpc.rs`
    /// owns the other half — what the surface actually answers — because that needs a live
    /// pane, which is exactly why this test could never have proved it.
    #[test]
    fn the_cells_family_declares_the_wire_words_it_uses() {
        // The template IS the definition; pin it verbatim.
        assert_eq!(CELLS_FIELD.path, "cells.<offset>");
        // What a `query` impl strips, and what `cells_slot_at` builds from.
        assert_eq!(CELLS_FIELD.literal_prefix(), "cells.");
        assert_eq!(cells_slot_at(7), "cells.7");
        // The declared argument, and the count path that bounds it — `frames`, which this
        // surface must actually serve for the domain to be true (`rpc.rs` proves it does).
        assert_eq!(CELLS_FIELD.args.len(), 1);
        assert_eq!(CELLS_FIELD.args[0].name, "offset");
        assert!(matches!(
            CELLS_FIELD.args[0].domain,
            ArgDomain::IndexOf(FRAMES_SLOT)
        ));
    }

    #[test]
    fn mux_action_path_matches_the_documented_grammar() {
        assert_eq!(mux_action_path(SPAWN_ACTION), "/sprag_mux/external/spawn");
        assert_eq!(mux_action_path(PANES_SLOT), "/sprag_mux/external/panes");
    }

    #[test]
    fn pane_container_tag_is_the_scene_assembly_tag() {
        // The head of a pane-addressed path must equal the container tag the scene
        // assembly stamps, or an invoke would address a non-existent node.
        assert_eq!(pane_container_tag(7), "pane_7");
        assert!(pane_input_path(7, KEY_ACTION).starts_with("/pane_7/"));
    }
}
