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

use crate::{INPUT_TAG, MUX_TAG};

/// The pane-input external invoke action that injects a key (W3C key + mods →
/// PTY bytes, the R2.6 encoder).
pub const KEY_ACTION: &str = "key";
/// The pane-input external invoke action that writes literal UTF-8 (IME commit /
/// paste), no key-encoding.
pub const TEXT_ACTION: &str = "text";
/// The pane-input external invoke action returning the pane's cell FRAME
/// ([`CellFrame`](crate::CellFrame)) at a SCROLLBACK offset — a user-driven history read.
/// **`offset == 0` is REFUSED**: the live view is [`LIVE_FRAME_SLOT`].
///
/// That split is deliberate, and enforced rather than merely documented. A display client
/// re-reads the live frame every time the scene revision moves (the `scene/waitFor` poll
/// loop), and an invoke is a `MethodOcc::Mutate` that bumps the revision — so serving the
/// live frame here made every read wake the very waiter it answered: a ~30Hz idle livelock
/// that burned a full core (R152). A query is a `MethodOcc::Read` and bumps nothing, so the
/// loop parks until real output. Scrollback stays an action only because it carries an
/// ARGUMENT, which pinion's `scene/query` cannot take (PINION-PR61 asks for a parameterized
/// read, which would collapse these two names back into one).
///
/// A scrollback read therefore still bumps, and that is not free: it wakes every OTHER
/// attached client's parked `waitFor` into a full re-fetch. It is bounded (a wheel tick, not
/// a poll) and terminates, so it is not the livelock — but it is the same defect, and only
/// PR-61 removes it.
pub const CELLS_ACTION: &str = "cells";
/// The pane-input external query slot: the pane's LIVE cell FRAME
/// ([`CellFrame`](crate::CellFrame)) — the `offset == 0` view a display client
/// projects each frame. A READ (see [`CELLS_ACTION`] for why the live view is a
/// slot, not an action): it bumps no revision, so the wire client's `scene/waitFor`
/// poll loop does not self-wake by reading it.
pub const LIVE_FRAME_SLOT: &str = "frame";
/// The pane-input external query slot: the pane's full output text (scrollback +
/// visible).
pub const FULL_TEXT_SLOT: &str = "full_text";
/// The pane-input external query slot: the producer's DECCKM (application cursor
/// keys) mode.
pub const CURSOR_KEYS_SLOT: &str = "application_cursor_keys";

/// The mux control external invoke action that spawns a pane, returning its id.
pub const SPAWN_ACTION: &str = "spawn";
/// The mux control external invoke action that closes a pane.
pub const CLOSE_ACTION: &str = "close";
/// The mux control external invoke action that resizes a pane's PTY + emulator.
pub const RESIZE_ACTION: &str = "resize";
/// The mux control external query slot: the live pane list as JSON.
pub const PANES_SLOT: &str = "panes";
/// The mux control external query slot: the current window's LOGICAL layout + the
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_input_path_matches_the_documented_grammar() {
        assert_eq!(
            pane_input_path(0, KEY_ACTION),
            "/pane_0/sprag_input/external/key"
        );
        assert_eq!(
            pane_input_path(3, CELLS_ACTION),
            "/pane_3/sprag_input/external/cells"
        );
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
