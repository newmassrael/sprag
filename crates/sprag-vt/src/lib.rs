//! sprag-vt — the VT port and its termwiz-backed emulator.
//!
//! DESIGN.md §4: termwiz is the embedded verified escape parser; the
//! emulator state machine (cursor, SGR pen, scroll, erase, alt-screen)
//! is sprag-owned. [`port`] is the library-agnostic seam; [`emulator`]
//! is the termwiz adapter that fills a [`port::Screen`].

pub mod emulator;
pub(crate) mod history;
pub mod port;

pub use emulator::{Emulator, osc52_reply};
pub use port::{
    Attrs, Cell, ClipboardQuery, ClipboardTarget, ClipboardTargets, ClipboardWrite, Color,
    ColorTarget, Cursor, CursorShape, FIND_MATCH_CAP, FindLine, FindMatch, FindResult, Hyperlink,
    Image, InputModes, KittyKeyboardFlags, LastCommand, LinkRun, MouseEncoding, MouseProtocol,
    Notification, Palette, PromptMark, Rgb, Screen, ScreenKind, ShellState, UnderlineStyle, VtPort,
    Width, char_columns,
};
