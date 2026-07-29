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
//! [`grid_changes`] turns cells into [`Change`](termwiz::surface::Change)s, and a binary owns a
//! terminal to write them to.
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

mod paint;

pub use paint::{cell_attributes, grid_changes};
