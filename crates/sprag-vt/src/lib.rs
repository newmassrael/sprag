//! sprag-vt — the VT port and its termwiz-backed emulator.
//!
//! DESIGN.md §4: termwiz is the embedded verified escape parser; the
//! emulator state machine (cursor, SGR pen, scroll, erase, alt-screen)
//! is sprag-owned. [`port`] is the library-agnostic seam; [`emulator`]
//! is the termwiz adapter that fills a [`port::Screen`].

/// Declaring an enum together with the array of every one of its variants, so the two cannot drift.
/// A plain `//` on the module below: an outer doc on a `mod` merges with the module's own `//!`.
pub mod closed_set;
pub mod emulator;
pub mod history;
pub mod port;

pub use emulator::{Emulator, osc52_reply};
pub use history::HistoryLimits;
pub use port::{
    Attrs, BadPattern, Cell, ClipboardQuery, ClipboardTarget, ClipboardTargets, ClipboardWrite,
    Color, ColorTarget, Cursor, CursorShape, DEFAULT_SCROLLBACK_LINES, FIND_MATCH_CAP, FindLine,
    FindMatch, FindResult, Hyperlink, Image, InputModes, KittyKeyboardFlags, LastCommand, LinkRun,
    MouseEncoding, MouseProtocol, Notification, Palette, PromptMark, REGEX_SIZE_LIMIT, Rgb, Screen,
    ScreenKind, ShellState, UnderlineStyle, Urgency, VtPort, Width, char_columns,
};
