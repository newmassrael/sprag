//! `sprag-tui` — sprag's terminal frontend.
//!
//! The second of sprag's two display clients. [`sprag_gui`] paints a session's panes as pixels
//! through a GPU; this one paints them as escape sequences into whatever terminal it was started
//! in — which is what makes `ssh host sprag attach --tui` possible, and what every rival
//! multiplexer already had.
//!
//! [`sprag_gui`]: https://github.com/newmassrael/sprag
//!
//! # What is here and what is not
//!
//! A display client is two things: a relationship with a `sprag-term` host, and a UNIT to draw it
//! in. The first half is [`sprag_client`] and is shared verbatim with the GUI — the same
//! [`WireHost`](sprag_client::WireHost), the same [`HostClient`](sprag_host::HostClient) protocol,
//! the same addresses. This crate is only the second half, and it is deliberately small:
//! [`tile`] says which cells belong to which pane, [`pane_changes`] turns those cells into
//! [`Change`](termwiz::surface::Change)s, [`wire_key`] turns this terminal's keystrokes back into
//! the names the wire carries, and a binary owns a terminal to do all three against.
//!
//! Output and input are inverses of each other in spirit — one carries a pane's cells OUT to a
//! screen, the other carries a user's keystrokes IN to a pane — and between them sits the only
//! thing a MULTI-pane client needs that a single-pane one does not: an answer to "which pane is
//! this cell", which is [`tile`]. That is the whole of what a terminal frontend adds to a client
//! that already knows how to talk to a host.
//!
//! That split is the reason `sprag-client` was extracted before any of this was written. A
//! terminal client that re-spelled the wire would be a second definition of an ABI whose whole
//! point is that there is one.
//!
//! # GPU-free BY GATE
//!
//! `tests/gpu_free.rs` asserts that this crate's resolved dependency closure contains no
//! `vello` / `wgpu` / `winit`. The property matters more here than anywhere else in the workspace:
//! this is the binary that runs on the headless box a user ssh-es into, so "it happens not to link
//! a GPU stack today" is not a property, it is a coincidence. The same gate guards
//! [`sprag_client`], and both are needed — a dependency added HERE would never reach that one.

mod key;
mod mouse;
mod paint;

pub use key::{WireKey, wire_key};
// The cell-space tiler moved to `sprag-terminal` so `sprag-gui` computes pane sizes through the
// SAME function (see `sprag_terminal::tiling`). Re-exported rather than dropped: this crate's
// callers ask a terminal frontend where a pane goes, and the answer's address is not their concern.
pub use mouse::MouseEdges;
pub use paint::{
    PaintCache, PanePaint, agent_window_title, cell_attributes, cursor_changes, divider_changes,
    help_changes, help_viewport, pane_changes, prompt_changes, title_change,
};
pub use sprag_terminal::{Divider, PaneRect, Rect, Tiling, tile, with_ratio};
