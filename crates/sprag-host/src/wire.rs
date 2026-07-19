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
    CELLS_FIELD,
    SchemaField::new(FRAMES_SLOT, "int"),
    SchemaField::new(CURSOR_KEYS_SLOT, "bool"),
    SchemaField::new(FULL_TEXT_SLOT, "string"),
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

/// The mux control external query slot: every session's name, plus which one an unscoped
/// request acts on — how a client discovers what it can address with [`SESSION_PARAM`].
///
/// Served rather than left to convention: a scope param an agent cannot enumerate is one it
/// must guess at, and the whole point of the scene-as-data surface is that a peer ASKS.
pub const SESSIONS_SLOT: &str = "sessions";

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
/// (`{tree, expected_revision}`), returning the canonical
/// [`LayoutSnapshot`](sprag_terminal::LayoutSnapshot).
///
/// `expected_revision` is the revision the gesture was authored against — a compare-and-set,
/// so a write against an arrangement that has moved on is REFUSED rather than silently
/// reverting whoever moved it.
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
