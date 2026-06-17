//! sprag-terminal — the PTY producer.
//!
//! Owns the OS pseudoterminal and the [`sprag_vt::Emulator`] it feeds,
//! exposing the live terminal [`sprag_vt::Screen`] as queryable state. This
//! is the producer side of the walking-skeleton slice (DESIGN.md §5):
//!
//! ```text
//! PTY ─▶ termwiz parser ─▶ sprag emulator ─▶ queryable Screen
//! ```
//!
//! Scene assembly and the scene-as-data RPC server live one layer up, in
//! `sprag-host`, so this crate depends only on the emulator and the PTY
//! abstraction — never on pinion (DESIGN.md §3: the producer owns state; the
//! host projects it).

pub mod session;

pub use session::{CommandBuilder, SessionError, TerminalSession};
