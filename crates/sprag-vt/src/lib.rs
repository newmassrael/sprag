//! sprag-vt — the VT port and its termwiz-backed emulator.
//!
//! DESIGN.md §4: termwiz is the embedded verified escape parser; the
//! emulator state machine (cursor, SGR pen, scroll, erase, alt-screen)
//! is sprag-owned. [`port`] is the library-agnostic seam; [`emulator`]
//! is the termwiz adapter that fills a [`port::Screen`].

pub mod emulator;
pub mod port;

pub use emulator::Emulator;
pub use port::{
    Attrs, Cell, Color, Cursor, CursorShape, InputModes, Rgb, Screen, ScreenKind, VtPort, Width,
};
