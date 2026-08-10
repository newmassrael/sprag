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
// Emulator-INTERNAL device state, the peer of `port::ScrollRegion`: no tab stop ever reaches the
// projection, the wire or a client, so nothing outside this crate can have an opinion about one.
// A plain `//` for the reason given above `closed_set` — an outer doc here would merge into the
// module's own `//!` and be resolved in THIS scope, where its types are not in scope at all.
pub(crate) mod tabstops;
/// Projecting a closed set's variants through its own spelling into the array of wire words a
/// schema can publish. A plain `//` on the module below, for `closed_set`'s reason.
pub mod wire_words;

pub use emulator::{Emulator, osc52_reply};
pub use history::HistoryLimits;
pub use port::{
    Attrs, BadPattern, Cell, ClipboardQuery, ClipboardTarget, ClipboardTargets, ClipboardWrite,
    Color, ColorTarget, Cursor, CursorShape, DEFAULT_SCROLLBACK_LINES, FIND_MATCH_CAP, FindLine,
    FindMatch, FindResult, Hyperlink, Image, InputModes, KittyKeyboardFlags, LastCommand, LinkRun,
    MouseEncoding, MouseProtocol, Notification, Palette, PromptMark, REGEX_SIZE_LIMIT, Rgb, Screen,
    ScreenKind, ShellState, UnderlineStyle, Urgency, VtPort, Width, char_columns,
};
