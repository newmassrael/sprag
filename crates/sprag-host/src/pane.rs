//! The terminal pane as a pinion `External` — the engine side of the
//! R1.7 split.
//!
//! PINION-REQUIREMENTS R1.7 separates the pane into *data* and *engine*:
//! the cell grid is exposed as a `Scene::TextGrid` (introspectable
//! projection), while the PTY+emulator engine sits behind an `External`
//! boundary (process-side opacity justified). [`SpragPaneExternal`] is that
//! engine surface. It carries no scene state of its own — only a
//! [`PanePtyHandle`] onto the live producer — so input is a *producer*
//! mutation reached through pinion's canonical `scene/invoke`, not a
//! mutation of pinion's projection (R969: pinion projects, the producer
//! owns state).
//!
//! The action channel is the R2.6 input seam: `invoke("key", {key, …})`
//! encodes the W3C key + modifiers to PTY bytes ([`sprag_input::encode`],
//! sprag-owned) and writes them to the child. A sibling `invoke("text",
//! {text})` writes **literal** UTF-8 to the child (no key-encoding) — the seam
//! for IME-composed input (a Hangul/CJK commit is text, not a keystroke); the
//! AI peer drives the same wire. `invoke("paste", {text})` is the paste seam:
//! like `text`, but the host brackets it (`ESC [ 200 ~` … `ESC [ 201 ~`) when
//! the child enabled DEC private mode 2004, so a multi-line paste is held as
//! one paste, not executed line by line.
//!
//! The read channel serves the pane's cell FRAME as the `query("cells.<offset>")` family
//! ([`CELLS_FIELD`]) — the projected [`GridBuffer`] at that scrollback offset (serde-able
//! since PINION-PR49) plus the scroll facts (scrollback depth + visible rows) that ride with
//! it — the wire display client's per-frame read (topology B: the client reconstructs the
//! exact buffer the host projected and paints it, so "read data, not pixels" reaches the
//! human path). The argument RIDES THE PATH, which is what makes a frame read a read: this
//! was an `invoke` while sprag believed the path-only `scene/query` could not carry an
//! offset, and PINION-PR61 established it never could not (`width.<col>` and `id_at.<pos>`
//! were already doing it upstream). An invoke is a `MethodOcc::Mutate`, so that belief cost a
//! ~30Hz idle livelock and, after R152 half-fixed it, a wheel tick that woke every other
//! attached client. It also exposes the producer-owned input modes
//! (`query("application_cursor_keys")`) and the pane's full output text
//! (`query("full_text")`, scrollback + visible) — the same `Screen::full_text` the in-process
//! capture path reads, so an external peer and a plugin share one notion of the screen.

use std::fmt;

use pinion_core::GridBuffer;
use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
    read_only_or_unknown,
};
use serde_json::{Value, json};
use sprag_input::{KeyEdge, Modifiers, MouseButton, MouseEventKind, MouseInput};
use sprag_terminal::PanePtyHandle;
use sprag_vt::{ClipboardTarget, Screen, osc52_reply};

use crate::external::{declined, refused, rpc_external_impl};
use crate::host::PaneScrollFacts;

/// The refusal every write to a pane's terminal shares: the bytes were formed and the child would
/// not take them.
///
/// A `const` because six actions state it and one situation deserves one sentence — the rule
/// [`crate::external::refused`] exists to keep. It is deliberately about the CHILD rather than
/// about the request: nothing the caller sends differently reaches a pane whose program has gone.
const NOT_WRITTEN: &str = "the pane's terminal would not take the write";

// The action names + query slots this external answers are the shared wire ABI
// vocabulary ([`crate::wire`]) — the SAME consts the wire client addresses, so the
// two cannot drift.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::wire::{
    ACTION_GRAMMAR_SLOT, ActionGrammar, CELLS_FIELD, CLIPBOARD_ANSWER_ACTION, CLIPBOARD_WRITE_SLOT,
    CURSOR_KEYS_SLOT, FIND_FIELD, FOCUS_ACTION, FRAMES_SLOT, FULL_TEXT_SLOT, IMAGE_DATA_FIELD,
    KEY_ACTION, LAST_COMMAND_SLOT, LINKS_SLOT, MOUSE_ACTION, PANE_GRAMMAR, PANE_SCHEMA,
    PASTE_ACTION, PROMPT_MARKS_SLOT, REGEX_FIELD, TEXT_ACTION,
};

/// Search `screen`'s retained output for the LITERAL `needle` — the one place the
/// `find.<needle>` language is bound to an engine call.
///
/// It is a function rather than two lines inlined at the slot because it has TWO callers with very
/// different cadences: the query slot (a find bar's keystroke) and the output-wait pass
/// (`crate::rpc`, once per coalesced burst of a watched pane's output). A round whose thesis is that
/// "does it say X" and "wait until it says X" are one semantics cannot afford two places deciding
/// which engine a language reaches — the shared [`crate::PaneFind`] makes the ANSWER one shape, and
/// this makes the QUESTION one mapping.
pub(crate) fn search_literal(screen: &Screen, needle: &str) -> crate::PaneFind {
    crate::PaneFind::from_screen_result(&screen.find(needle))
}

/// The same search read as a REGULAR EXPRESSION — [`search_literal`]'s peer, and separate for the
/// reason [`crate::wire::REGEX_FIELD`] gives: a needle and a pattern are separate languages.
///
/// A pattern the engine refuses answers the normal shape carrying its message rather than an
/// absence, because an invalid pattern is a well-formed question whose VALUE was rejected.
pub(crate) fn search_pattern(screen: &Screen, pattern: &str) -> crate::PaneFind {
    match screen.find_regex(pattern) {
        Ok(found) => crate::PaneFind::from_screen_result(&found),
        Err(bad) => crate::PaneFind::refused(&bad),
    }
}

/// Why a key never reached a pane's child — [`send_key`]'s refusal.
///
/// Two facts, and this type exists because they were ONE `bool` until R325. The two send a caller
/// somewhere completely different: an unencodable key is a request this build cannot express and a
/// failed write is a child that is gone, and a surface handed `false` had to name both or neither.
/// It is the same fusion the whole round removes one layer up, met inside sprag's own code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyUnsent {
    /// [`sprag_input::encode`] has no bytes for that key + modifier combination under the pane's
    /// live input modes — a name this build's encoder does not know.
    Unencodable,
    /// The bytes were encoded and the pane's terminal would not take them; its child has gone.
    NotWritten,
}

impl std::fmt::Display for KeyUnsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // NAMES THE VOCABULARY, because the encoder is the only thing that knows it. The
            // `sprag` CLI used to write this sentence itself — it had to, since a payload-free
            // refusal could not carry one — and a client authoring a claim about another process's
            // encoder is what this round removes everywhere else too.
            Self::Unencodable => f.write_str(
                "not a key this build encodes — a key is a W3C key name (Enter, Escape, Tab, \
                 ArrowUp, F5) or a single character, optionally prefixed C- / M- / S-",
            ),
            Self::NotWritten => f.write_str("the pane's terminal would not take the keystroke"),
        }
    }
}

impl std::error::Error for KeyUnsent {}

/// Encode a W3C `key` + `mods` to PTY bytes (the sprag-owned R2.6 encoder,
/// [`sprag_input::encode`]) and write them to `pty`, answering WHICH way it failed.
///
/// This is the key->PTY SSOT shared by the RPC input surface
/// ([`SpragPaneExternal`]'s `key` action, which parses the JSON/scene wire) and the
/// in-process display client ([`HostClient::send_key`](crate::HostClient::send_key), which calls
/// this directly with typed args) — so the human keyboard path and the AI
/// `scene/invoke` path encode IDENTICALLY.
///
/// A [`KeyUnsent`] rather than `false`: see that type for why the two causes had to come apart. A
/// caller that only needs "did it go" writes `.is_ok()`, which is what the display clients do.
pub fn send_key(pty: &PanePtyHandle, key: &str, mods: Modifiers) -> Result<(), KeyUnsent> {
    let bytes = sprag_input::encode(key, mods, pty.input_modes()).ok_or(KeyUnsent::Unencodable)?;
    pty.write(&bytes)
        .map(|_| ())
        .map_err(|_| KeyUnsent::NotWritten)
}

/// Write literal UTF-8 `text` to `pty` (no key-encoding) — the IME-commit /
/// paste seam. Empty text is a no-op success. `true` on success; `false` on a
/// write failure. The text->PTY SSOT shared by [`SpragPaneExternal`]'s `text`
/// action and the in-process client.
#[must_use]
pub fn send_text(pty: &PanePtyHandle, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    pty.write(text.as_bytes()).is_ok()
}

/// The bracketed-paste START marker (`ESC [ 200 ~`) — written before pasted text when the pane's
/// child has enabled DEC private mode 2004.
const PASTE_BRACKET_START: &str = "\x1b[200~";
/// The bracketed-paste END marker (`ESC [ 201 ~`) — written after pasted text.
const PASTE_BRACKET_END: &str = "\x1b[201~";

/// PASTE literal UTF-8 `text` into `pty` — the clipboard-paste seam, distinct from [`send_text`]
/// (typed / IME-committed text). When the pane's child has enabled bracketed paste (DEC private
/// mode 2004, read LIVE from the emulator), the text is wrapped in `ESC [ 200 ~` … `ESC [ 201 ~`
/// so the child can tell a paste from typed keystrokes (a shell / editor then holds a multi-line
/// paste instead of executing each line). Otherwise it is written raw, exactly like [`send_text`].
///
/// The bracketing decision lives HERE, at the PTY boundary, because the emulator holds the
/// authoritative mode — the same reason [`send_key`] encodes here rather than in the display
/// client (which never sees [`crate::HostClient`]-side input modes).
///
/// Security: any embedded END marker in the pasted text is stripped BEFORE wrapping, so a paste
/// whose content contains `ESC [ 201 ~` cannot close the bracket early and have its tail
/// interpreted as typed commands (the paste-injection guard xterm applies). Empty text is a
/// no-op success. `true` on success; `false` on a write failure.
#[must_use]
pub fn paste(pty: &PanePtyHandle, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if !pty.input_modes().bracketed_paste {
        return pty.write(text.as_bytes()).is_ok();
    }
    pty.write(&frame_bracketed_paste(text)).is_ok()
}

/// REPORT a mouse event to `pty`'s child — the mouse-tracking seam, distinct from [`send_key`] /
/// [`send_text`]. The semantic event (a cell + a button edge) is gated against the pane's active
/// mouse-tracking mode and encoded to an X10 / SGR report by the sprag-owned encoder
/// ([`sprag_input::encode_mouse`], reading the LIVE modes from the emulator). Like [`send_key`], the
/// encoding lives HERE at the PTY boundary because the emulator holds the authoritative mode; a
/// display client supplies only the raw cell + button.
///
/// An event the active mode does not want (no tracking on, a motion outside any-event tracking, a
/// drag outside button/any-event tracking) is a no-op SUCCESS — it is silently dropped, not a
/// failure, exactly as an empty [`send_text`] is. `true` on a successful write or a legitimate drop;
/// `false` only on a write failure.
#[must_use]
pub fn mouse(pty: &PanePtyHandle, event: MouseInput) -> bool {
    match sprag_input::encode_mouse(event, pty.input_modes()) {
        Some(bytes) => pty.write(&bytes).is_ok(),
        None => true, // the mode did not want this event — a no-op, not a rejection
    }
}

/// Report a pane FOCUS change to its child, gated + encoded against the pane's live focus-reporting
/// mode ([`sprag_input::encode_focus`]): `ESC [ I` on focus gained, `ESC [ O` on focus lost, only
/// while the child has enabled DEC private mode 1004. The mode authority lives HERE at the PTY
/// boundary (like [`mouse`] / key encoding); a display client reports only the edge. A change the
/// mode does not want (1004 off) is a no-op SUCCESS, not a failure. `true` on a successful write or
/// a legitimate drop; `false` only on a write failure.
#[must_use]
pub fn focus(pty: &PanePtyHandle, focused: bool) -> bool {
    match sprag_input::encode_focus(focused, pty.input_modes()) {
        Some(bytes) => pty.write(&bytes).is_ok(),
        None => true, // focus reporting is off — a no-op, not a rejection
    }
}

/// Frame `text` as a bracketed paste: `ESC [ 200 ~` + `text` (with any embedded bracket marker
/// filtered out) + `ESC [ 201 ~`. Pure (no PTY) so the framing and the paste-injection guard are
/// deterministically testable. The END marker is filtered so a paste whose content contains
/// `ESC [ 201 ~` cannot close the bracket early and have its tail read as typed commands; the
/// START marker is filtered too (a forged start only confuses a child that already saw ours).
fn frame_bracketed_paste(text: &str) -> Vec<u8> {
    let sanitized = text
        .replace(PASTE_BRACKET_END, "")
        .replace(PASTE_BRACKET_START, "");
    let mut framed = Vec::with_capacity(PASTE_BRACKET_START.len() + sanitized.len() + 6);
    framed.extend_from_slice(PASTE_BRACKET_START.as_bytes());
    framed.extend_from_slice(sanitized.as_bytes());
    framed.extend_from_slice(PASTE_BRACKET_END.as_bytes());
    framed
}

/// The pane engine `External`: a thin, scene-stateless forwarder onto the
/// live [`PanePtyHandle`]. Input arrives via `scene/invoke` and is encoded
/// to PTY bytes by the sprag-owned encoder (R2.6); the producer's reader
/// thread lives behind this boundary, so the engine is `UiThreadSync` from
/// pinion's vantage (it does its work synchronously when invoked).
pub struct SpragPaneExternal {
    pty: PanePtyHandle,
}

impl SpragPaneExternal {
    /// Build the engine surface over a live pane's PTY I/O handle.
    #[must_use]
    pub fn new(pty: PanePtyHandle) -> Self {
        Self { pty }
    }

    /// Encode a `key` action's args and write the resulting bytes to the
    /// PTY. A `state:"up"` edge is a no-op success (terminals emit no
    /// release in this mode). An unencodable key or a write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_key(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let Some((key, mods)) = parse_key_args(args)? else {
            return Ok(IntrospectValue::Null); // suppressed key-up edge
        };
        send_key(&self.pty, &key, mods)
            .map(|()| IntrospectValue::Null)
            .map_err(refused)
    }

    /// Write a `text` action's literal UTF-8 to the PTY — **not** key-encoded.
    /// This is the seam for IME-composed input (a Hangul/CJK
    /// [`CompositionEvent::Commit`](pinion_core::CompositionEvent) is finished
    /// text, not a keystroke) and for pasting. Empty text is a no-op success
    /// (the IME's cancel-via-empty-commit shape). A write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_text(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let text = parse_text_args(args)?;
        if send_text(&self.pty, &text) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// PASTE a `paste` action's literal UTF-8 into the PTY — like [`Self::inject_text`], but the
    /// text is bracketed (and its embedded end marker filtered) when the child enabled DEC private
    /// mode 2004. The seam a display client's clipboard paste reaches over the wire. Empty text is
    /// a no-op success. A write failure is an [`InvokeError::Rejected`].
    fn inject_paste(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let text = parse_text_args(args)?;
        if paste(&self.pty, &text) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Parse a `mouse` action's args into a [`MouseInput`] and report it to the PTY, gated + encoded
    /// against the pane's live mouse-tracking mode ([`mouse`]). An event the mode drops is a no-op
    /// success; a write failure is an [`InvokeError::Rejected`].
    fn inject_mouse(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let event = parse_mouse_args(args)?;
        if mouse(&self.pty, event) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Parse a `focus` action's args (`{focused: bool}`) and report the edge to the PTY, gated +
    /// encoded against the pane's live focus-reporting mode ([`focus`]). A change 1004 does not want
    /// is a no-op success; a write failure is an [`InvokeError::Rejected`].
    fn inject_focus(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let focused = parse_focus_args(args)?;
        if focus(&self.pty, focused) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Answer a pending OSC 52 read query: format the `OSC 52` reply for the requested selection
    /// and hand it to the pane's exactly-once arbiter, which writes it to the PTY only if no
    /// client answered this query (or a newer one) first. Returns `{wrote}` — whether THIS answer
    /// reached the PTY — so a caller can tell it lost the race (harmless; the child got its
    /// reply). A write failure is an [`InvokeError::Rejected`].
    fn answer_clipboard(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let (seq, target, text) = parse_clipboard_answer_args(args)?;
        let reply = osc52_reply(target, &text);
        match self.pty.answer_clipboard_query(seq, &reply) {
            Ok(wrote) => Ok(IntrospectValue::Json(json!({ "wrote": wrote }))),
            Err(_) => Err(refused(NOT_WRITTEN)),
        }
    }

    /// The pane's cell FRAME at scrollback `offset` — the wire display client's
    /// per-frame read (topology B). A JSON-able struct:
    ///
    /// * `cells` — the projected [`GridBuffer`]
    ///   ([`sprag_grid::project_scrolled`], serde-able since PINION-PR49), the
    ///   paint-authoritative buffer the client reconstructs byte-for-byte;
    /// * `scrollback_len` — the retained history depth (the scrollbar extent + the
    ///   top-anchored offset math);
    /// * `visible_rows` — one scrollback page.
    ///
    /// The [`GridBuffer`] and the [`PaneScrollFacts`] are read under ONE screen lock —
    /// an atomically consistent snapshot (the cells and the scroll facts describe the
    /// SAME screen state, never a torn read across two locks). The facts flatten into
    /// the frame from the ONE [`PaneScrollFacts`] type (its field names ARE the wire
    /// keys), read through [`PaneScrollFacts::from_screen`] — the same population the
    /// in-process [`HostClient::pane_scroll_facts`](crate::HostClient::pane_scroll_facts)
    /// uses, so the two clients cannot disagree on the frame's non-cell shape.
    ///
    /// Served at every offset through the ONE [`CELLS_FIELD`] query family: `offset == 0` is
    /// the live view and a larger offset windows into history, self-clamping to the retained
    /// depth (so `0..=scrollback_len` are all answerable, and past the top gets the top).
    fn frame_at(&self, offset: usize) -> CellFrame {
        self.pty.with_screen_palette(|screen, palette| CellFrame {
            cells: sprag_grid::project_scrolled(screen, offset, palette),
            facts: PaneScrollFacts::of(screen, offset),
        })
    }
}

/// The wire frame the [`CELLS_FIELD`] family answers: the projected paint buffer plus the
/// non-cell [`PaneScrollFacts`] that ride with it, serialized as one flat JSON object
/// (`{cells, scrollback_len, visible_rows}`). `#[serde(flatten)]` pulls the facts'
/// field names up as the wire keys, so the frame's non-cell keys are defined ONCE
/// (on [`PaneScrollFacts`]) rather than re-listed here.
///
/// This is the ONE definition of the whole pane frame's wire shape — the envelope
/// (the `cells` key + its [`GridBuffer`] type) AND the flattened facts — with BOTH
/// `Serialize` (the host `query` end) and `Deserialize` (the wire client end,
/// `sprag-gui`'s `WireHost`). A single type owned by this crate, so a field rename
/// on either end is a compile error, not a silent runtime divergence (the exact
/// SSOT the R116 review established for the facts, here extended to the envelope).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CellFrame {
    /// The projected paint-authoritative cell buffer (serde-able since PINION-PR49), written in
    /// [`sprag_grid::wire`]'s run-length form rather than through its derived `Serialize`.
    ///
    /// The derived shape spells every cell in full — R221 measured it at **297 bytes per cell,
    /// 570,583 for one 80x24 pane** — and the reply's size is what the request's time is made of:
    /// building the `serde_json::Value` for it cost 2.2-3.0 ms of a ~4 ms fetch, because a DOM's
    /// cost is its node count. The attribute changes only how the field crosses the wire; the
    /// field is the same [`GridBuffer`] to every reader on both ends, so nothing downstream of
    /// here knows the difference.
    #[serde(with = "sprag_grid::wire")]
    pub cells: GridBuffer,
    /// The non-cell per-frame facts, flattened so `scrollback_len` / `visible_rows`
    /// are top-level wire keys (their names come from [`PaneScrollFacts`], the SSOT).
    #[serde(flatten)]
    pub facts: PaneScrollFacts,
}

impl fmt::Debug for SpragPaneExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `PanePtyHandle` wraps un-`Debug` PTY/emulator handles; the engine
        // is identified structurally (External: Debug is required by pinion §5.2).
        f.debug_struct("SpragPaneExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(SpragPaneExternal);

impl ExternalIntrospect for SpragPaneExternal {
    fn schema(&self) -> IntrospectSchema {
        // Declared in `wire`, beside the addresses — a field's TYPE is part of its
        // declaration, and this surface's vocabulary has ONE home.
        IntrospectSchema::new(PANE_SCHEMA)
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        // The parametric family goes FIRST, before the exact-path arms: its argument rides
        // the path, so it is matched by prefix rather than by equality. Every frame read —
        // live and history alike — is a READ, so no client can wake the waiter it is
        // parked on merely by looking (the R152 livelock, and the wheel-tick bump that
        // outlived it).
        if let Some(arg) = path.strip_prefix(CELLS_FIELD.literal_prefix()) {
            // Stripping the DECLARED prefix is what makes a path a MEMBER of the family —
            // the same question `SchemaField::addresses` answers, and the reason a malformed
            // member is `Null` (present-but-empty) rather than `None`. `None` here becomes
            // `UnknownIntrospectPath`, whose definition is "not in its schema" — and
            // `cells.zzz` IS in the schema. pinion states the model on `addresses`
            // ("`width.zzz` belongs to `width` and is malformed, not unknown") and the
            // remedy on `at_index` ("reports `Null` … never absence").
            //
            // R155's review caught the first draft answering `None` and defending it as
            // "`query` has no error channel, and answering a plausible frame is the quiet
            // wrong answer" — a false dichotomy that skipped the third option pinion
            // specifies. Absence was the quiet wrong answer: it denied the surface owned an
            // address it advertises.
            // ENCODED ONCE, not built as a DOM and re-encoded. `IntrospectValue::raw` (pinion
            // R1480, delivering PINION-PR79) carries JSON TEXT the producer already holds, and
            // `scene/query` splices those bytes into the reply instead of walking a tree — so a
            // frame is serialized exactly once, here. The wire bytes are UNCHANGED; only the way
            // they are produced is. `Null` on a serialization failure is the same degradation
            // `to_value(..).map_or(Null, Json)` had, so the answer taxonomy above still holds.
            //
            // This is the answer R222 measured at 297 -> 5 B/cell; the DOM this removes was the
            // 25-30% of the request that survived that round, and the reason it could not be
            // removed then was upstream, not here.
            return Some(cells_offset(arg).map_or(IntrospectValue::Null, |offset| {
                IntrospectValue::raw(&self.frame_at(offset))
            }));
        }
        // Every literal match of a needle in the pane's retained output, read ON DEMAND (a find
        // bar's keystroke, never per frame). A READ — searching a pane changes nothing about it, so
        // a client that re-queries as the user types cannot wake the waiters it is parked beside
        // (the R152 lesson `cells` was moved off an invoke for). The needle rides the path verbatim;
        // an EMPTY one is a malformed member and answers `Null`, the same taxonomy `cells.<off>`
        // reports (present-but-empty, never absence — the path IS in the schema).
        if let Some(needle) = path.strip_prefix(FIND_FIELD.literal_prefix()) {
            if needle.is_empty() {
                return Some(IntrospectValue::Null);
            }
            let found = self
                .pty
                .with_screen(|screen| search_literal(screen, needle));
            // Serialized from the SHARED wire type, not a hand-built object: the client
            // deserializes that same type, so the keys are symmetric by construction. Encoded
            // once and spliced, for the reason the `cells` arm above states.
            return Some(IntrospectValue::raw(&found));
        }
        // The same search over a REGULAR EXPRESSION — a separate address because a needle and a
        // pattern are separate languages, so one string cannot be allowed to mean both depending on
        // a mode carried somewhere other than the address (see `REGEX_FIELD`). An EMPTY pattern is a
        // malformed member and answers `Null` exactly as an empty needle does; an INVALID one is
        // not malformed addressing but a rejected VALUE, so it answers the normal shape carrying
        // the engine's message — `Null` there would read as "no such pane".
        if let Some(pattern) = path.strip_prefix(REGEX_FIELD.literal_prefix()) {
            if pattern.is_empty() {
                return Some(IntrospectValue::Null);
            }
            let found = self
                .pty
                .with_screen(|screen| search_pattern(screen, pattern));
            return Some(IntrospectValue::raw(&found));
        }
        // One inline image's RGBA as base64, fetched ON DEMAND (R1404 Stage 5) — the RGBA can be
        // megabytes, so it does not ride the per-poll panes slot (only the `{id,seq}` summary
        // does). `Null` (present-but-empty, a member of the family) for an id the pane is not
        // showing or a malformed id — the same shape a malformed `cells.<off>` reports.
        if let Some(arg) = path.strip_prefix(IMAGE_DATA_FIELD.literal_prefix()) {
            return Some(
                arg.parse::<u32>()
                    .ok()
                    .and_then(|id| {
                        self.pty.with_screen(|s| {
                            s.images()
                                .iter()
                                .find(|im| im.id == id)
                                .map(|im| STANDARD.encode(&im.rgba))
                        })
                    })
                    .map_or(IntrospectValue::Null, IntrospectValue::Text),
            );
        }
        match path {
            // The count that bounds `cells.<offset>` (`IndexOf(FRAMES_SLOT)`): the live view
            // plus one per retained history line. An agent reads this scalar to learn where
            // history ends, instead of fetching whole cell grids to find out.
            FRAMES_SLOT => Some(IntrospectValue::Int(
                i64::try_from(self.pty.with_screen(Screen::scrollback_len)).unwrap_or(i64::MAX) + 1,
            )),
            CURSOR_KEYS_SLOT => Some(IntrospectValue::Bool(
                self.pty.input_modes().application_cursor_keys,
            )),
            FULL_TEXT_SLOT => Some(IntrospectValue::Text(
                self.pty.with_screen(Screen::full_text),
            )),
            // The last command sliced from the OSC 133 marks (scrollback + visible). `Null`
            // (present-but-empty) when no command has run under shell integration — the
            // agent then falls back to `full_text`, exactly as a malformed `cells.<off>` is
            // `Null` rather than absent.
            LAST_COMMAND_SLOT => Some(self.pty.with_screen(Screen::last_command).map_or(
                IntrospectValue::Null,
                |cmd| {
                    IntrospectValue::Json(json!({
                        "command": cmd.command,
                        "output": cmd.output,
                        "exit_status": cmd.exit_status,
                        "running": cmd.running,
                    }))
                },
            )),
            // The OSC 133 prompt positions (logical indices from the oldest line) a
            // jump-to-prompt scrolls to — a JSON array, read on demand not per frame.
            PROMPT_MARKS_SLOT => Some(IntrospectValue::Json(json!(
                self.pty.with_screen(|screen| screen.prompt_positions())
            ))),
            // The OSC-8 hyperlink runs on the visible grid — a JSON array of {text, uri, id}, read
            // on demand by `read_pane_links`. `[]` when the pane shows no links. The link's URI as
            // data, which tmux's `capture-pane` cannot give (it flattens OSC 8 to plain text).
            LINKS_SLOT => Some(IntrospectValue::Json(json!(
                self.pty
                    .with_screen(Screen::hyperlink_runs)
                    .iter()
                    .map(|run| json!({ "text": run.text, "uri": run.uri, "id": run.id }))
                    .collect::<Vec<_>>()
            ))),
            // The pane's most recent OSC 52 clipboard WRITE, fetched ON DEMAND when the write seq
            // in the pane list grows (the payload can be a whole paste, so it is not carried per
            // poll). `Null` (present-but-empty) when the child has written no clipboard — the
            // client then has nothing to apply, exactly as a malformed `cells.<off>` is `Null`.
            // HOW TO CALL THIS SURFACE'S SIX VERBS. Answered from the surface a client already holds
            // a path to, so the address it asked scopes the answer — see `ACTION_GRAMMAR_SLOT`. The
            // table is a `const`, so this costs one walk of it per ask and nothing per frame.
            ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(ActionGrammar::answer(PANE_GRAMMAR))),
            CLIPBOARD_WRITE_SLOT => {
                let (write, seq) = self.pty.clipboard_write();
                Some(write.map_or(IntrospectValue::Null, |w| {
                    IntrospectValue::Json(json!({
                        "targets": {
                            "clipboard": w.targets.clipboard,
                            "primary": w.targets.primary,
                        },
                        "text": w.text,
                        "seq": seq,
                    }))
                }))
            }
            _ => None,
        }
    }

    fn intervene(&mut self, path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // Nothing here is writable: input is an action (invoke `key`), and every slot is
        // producer-owned. But "not writable" and "not there" are different facts, and
        // saying the wrong one is pinion's §2 #7 lie — an agent told `UnknownPath` for
        // `full_text`, which it can plainly `query`, learns something false about the
        // surface. `read_only_or_unknown` answers from the SCHEMA (routing through
        // `SchemaField::addresses`, so `cells.0` reports read-only like any other member),
        // which is why the declaration belongs in one place. R1353 shipped this helper; the
        // first draft of R155 imported the vocabulary and kept the flat lie.
        Err(read_only_or_unknown(&self.schema(), path))
    }

    /// # A verb this surface does not DECLARE is a verb it does not run
    ///
    /// ⚠⚠ **This surface was the ODD ONE OUT.** R352 put [`declares_verb`](crate::wire::declares_verb)
    /// at the door of the mux surface and of the GUI's three, and left the pane's input surface
    /// dispatching six verbs without ever consulting [`PANE_SCHEMA`] — five surfaces guarding and one
    /// not, which is R330's rule pointing straight at it. Nothing was unreachable-but-dispatched here
    /// TODAY; what was missing is the property that it cannot become so, on the surface a keybinding
    /// drives in-process most often of all.
    ///
    /// The cost is one linear scan of a `&'static [SchemaField]` per action — paid per keystroke, not
    /// per frame, and the same scan is what makes a declared verb reachable at all, so an arm added
    /// here without a declaration is dead in the same edit.
    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        if !crate::wire::declares_verb(&self.schema(), path) {
            return Err(InvokeError::UnknownPath);
        }
        match path {
            KEY_ACTION => self.inject_key(&args),
            MOUSE_ACTION => self.inject_mouse(&args),
            FOCUS_ACTION => self.inject_focus(&args),
            TEXT_ACTION => self.inject_text(&args),
            PASTE_ACTION => self.inject_paste(&args),
            CLIPBOARD_ANSWER_ACTION => self.answer_clipboard(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Parse the `clipboard_answer` action's params `{seq, sel, text}`: the query `seq` a client
/// saw, the selection char it read (`c` clipboard / `p` primary), and that selection's current
/// text. Only the object form is accepted (this is a machine wire, never hand-typed). A missing
/// or ill-typed field is an [`InvokeError::TypeMismatch`]; an empty `text` is valid (an empty
/// clipboard reads back as the empty string).
fn parse_clipboard_answer_args(
    args: &IntrospectValue,
) -> Result<(u64, ClipboardTarget, String), InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    let seq = map
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or(InvokeError::TypeMismatch)?;
    // The selection's two words come from the type that owns them, which is what lets the pane
    // surface publish `sel`'s vocabulary instead of leaving a client to discover it from a reply.
    let target = map
        .get("sel")
        .and_then(Value::as_str)
        .and_then(ClipboardTarget::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let text = map
        .get("text")
        .and_then(Value::as_str)
        .ok_or(InvokeError::TypeMismatch)?
        .to_owned();
    Ok((seq, target, text))
}

/// Parse the `text` action's args into the literal string to write. Accepts a
/// bare string (`"한"`) or an object `{text: "한"}` (the AI/JSON wire). A
/// missing/non-string `text`, or a non-string/non-object arg, is an
/// [`InvokeError::TypeMismatch`]. Empty is allowed (the caller no-ops it).
fn parse_text_args(args: &IntrospectValue) -> Result<String, InvokeError> {
    match args {
        IntrospectValue::Text(text) => Ok(text.clone()),
        IntrospectValue::Json(Value::Object(map)) => map
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(InvokeError::TypeMismatch),
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse a [`CELLS_FIELD`] member's `<offset>` argument — canonical non-negative decimal
/// only. `None` means the member is MALFORMED (the caller answers `Null`), never that the
/// path is unknown.
///
/// **Canonical, because `cells.7` / `cells.007` / `cells.+7` were three addresses for one
/// frame** — shipped by R155's first draft (a bare `parse::<usize>`, which accepts both) in
/// the same commit whose test doc says "a tolerated alias is how a split grows back". The
/// retired JSON-arg parser could not express either spelling (JSON has no `007`), so
/// consuming PR-61 had quietly INTRODUCED the aliases it was meant to remove. One frame, one
/// address.
///
/// An integer too large for `usize` SATURATES rather than being called malformed: it means
/// exactly what every other past-the-top offset means, and [`frame_at`]'s projection clamps
/// it to the top. Answering `2^64-1` with the top frame while calling `2^64` malformed would
/// split one concept across two answers for no reason a caller could predict.
///
/// [`frame_at`]: SpragPaneExternal::frame_at
fn cells_offset(arg: &str) -> Option<usize> {
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None; // a sign, a space, or a non-digit: not this argument's type
    }
    if arg.len() > 1 && arg.starts_with('0') {
        return None; // `007` is `7` spelled a second way
    }
    Some(arg.parse::<usize>().unwrap_or(usize::MAX))
}

/// Parse the `key` action's args into a `(key, modifiers)` pair, or `None`
/// when the edge is a `state:"up"` release (which injects nothing).
///
/// Accepts either a bare string (`"a"` → that key, no modifiers, press) or
/// an object `{key, ctrl?, alt?, shift?, super?, state?}`. The `super` JSON
/// field maps to [`Modifiers::sup`]. Malformed args are an
/// [`InvokeError::TypeMismatch`].
fn parse_key_args(args: &IntrospectValue) -> Result<Option<(String, Modifiers)>, InvokeError> {
    match args {
        IntrospectValue::Text(key) if !key.is_empty() => {
            Ok(Some((key.clone(), Modifiers::default())))
        }
        IntrospectValue::Json(Value::Object(map)) => {
            let key = map
                .get("key")
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty())
                .ok_or(InvokeError::TypeMismatch)?;
            // THE EDGE IS A CLOSED SET, and it used to be two string literals here — the same place
            // `SplitDir`'s two words lived before R352b, with the same consequence: the vocabulary
            // had no definition the pane surface could publish. ⚠ A `state` PRESENT at the wrong
            // JSON type is refused rather than read as a press: `and_then(Value::as_str)` folded
            // `{"state": 4}` into the `None` arm, so a malformed edge was injected as a keystroke.
            let edge = if declined(map, "state") {
                KeyEdge::Down
            } else {
                match &map["state"] {
                    Value::String(word) => {
                        KeyEdge::from_wire(word).ok_or(InvokeError::TypeMismatch)?
                    }
                    _ => return Err(InvokeError::TypeMismatch),
                }
            };
            if !edge.injects() {
                return Ok(None);
            }
            Ok(Some((key.to_string(), parse_modifier_flags(map, true)?)))
        }
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse the `mouse` action's args `{button, kind, col, row, ctrl?, alt?, shift?}` into a
/// [`MouseInput`]. Only the object form is accepted (a machine wire, never hand-typed). `button` is
/// `left`/`middle`/`right`/`wheelup`/`wheeldown`/`wheelleft`/`wheelright`/`none`, `kind` is
/// `press`/`release`/`drag`/
/// `motion`, and `col`/`row` are 0-based cells. A missing/ill-typed field is an
/// [`InvokeError::TypeMismatch`].
fn parse_mouse_args(args: &IntrospectValue) -> Result<MouseInput, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    // THROUGH THE TYPE'S OWN VOCABULARY, both of them. These two matches used to be spelled out
    // here, opposite an identical pair in `sprag_client` — one vocabulary, two definitions, two
    // crates. `MouseButton::wire_str` records what that cost; what matters here is that the words
    // this admits are now the same array the pane surface PUBLISHES, so a client that reads the
    // grammar cannot be told a word this refuses.
    let word = |name: &str| map.get(name).and_then(Value::as_str);
    let button = word("button")
        .and_then(MouseButton::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let kind = word("kind")
        .and_then(MouseEventKind::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let coord = |name: &str| -> Result<u16, InvokeError> {
        map.get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)
    };
    Ok(MouseInput {
        button,
        kind,
        col: coord("col")?,
        row: coord("row")?,
        mods: parse_modifier_flags(map, false)?,
    })
}

/// The modifier flags of an input action's object form — `ctrl`, `alt`, `shift`, and `super` when
/// the action carries one — each ABSENT-or-`bool`.
///
/// # ⚠⚠ A MALFORMED FLAG USED TO BE READ AS `false`
///
/// Both parsers did `map.get(name).and_then(Value::as_bool).unwrap_or(false)`, which cannot tell a
/// key that is missing from one holding `"true"`, `1`, or `null` — so `{"key": "a", "ctrl": 1}` was
/// silently injected as an UNMODIFIED `a` and answered success. That is R352b's `report_agent`
/// defect exactly (a `name` dropped by `and_then(as_str)` where its siblings refused one), and
/// R330's odd-one-out rule catches it here too: `col` and `row` in the very same parser refuse a
/// malformed value.
///
/// Absent still means `false` — a call that says nothing about `ctrl` is not holding it, and that is
/// the shape every client sends. What is refused is a flag PRESENT at the wrong type, which no
/// well-formed caller sends and which the wire now advertises as a `bool`
/// ([`PANE_GRAMMAR`]). A declared argument the daemon
/// coerces instead of reading is what `a_declared_argument_is_one_the_daemon_reads` refuses to let
/// the grammar publish.
fn parse_modifier_flags(
    map: &serde_json::Map<String, Value>,
    with_super: bool,
) -> Result<Modifiers, InvokeError> {
    // ⚠ DECLINED means `false`, and declined includes an explicit `null`: a client whose language
    // serialises an absent optional that way is not holding the modifier, and refusing the call
    // would make every one of its keystrokes malformed. A flag present at any OTHER wrong type is
    // still refused — that is the coercion this parser exists to have stopped doing.
    let flag = |name: &str| -> Result<bool, InvokeError> {
        if declined(map, name) {
            return Ok(false);
        }
        match &map[name] {
            Value::Bool(held) => Ok(*held),
            _ => Err(InvokeError::TypeMismatch),
        }
    };
    Ok(Modifiers {
        ctrl: flag("ctrl")?,
        alt: flag("alt")?,
        shift: flag("shift")?,
        // A mouse report has no encoding for the logo key, so that action does not read the key at
        // all — and a surface that does not read a key does not publish one either. Spelled as an
        // `if` rather than `with_super && flag(..)?` so it is visible that the flag is not READ
        // there, instead of being read and discarded.
        sup: if with_super { flag("super")? } else { false },
    })
}

/// Parse a `focus` action's args `{focused: bool}` into the focus edge. Only the object form is
/// accepted (a machine wire, never hand-typed); a missing/ill-typed `focused` is an
/// [`InvokeError::TypeMismatch`].
fn parse_focus_args(args: &IntrospectValue) -> Result<bool, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    map.get("focused")
        .and_then(Value::as_bool)
        .ok_or(InvokeError::TypeMismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_args(v: serde_json::Value) -> IntrospectValue {
        IntrospectValue::Json(v)
    }

    /// A quiescent pane and the surface over it — a child that blocks on its PTY and never
    /// writes, so the frame these tests read cannot change under them.
    fn surface() -> (sprag_terminal::Workspace, SpragPaneExternal) {
        let mut workspace = sprag_terminal::Workspace::new((20, 4));
        let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("exec cat");
        command.env("TERM", "dumb");
        let id = workspace
            .spawn(command, "cat".to_owned(), 20, 4)
            .expect("a pane spawns");
        let external = SpragPaneExternal::new(
            workspace
                .pane(id)
                .expect("the pane is there")
                .pty()
                .handle(),
        );
        (workspace, external)
    }

    /// THE ENCODING ITSELF — the one claim no other test in this crate can see.
    ///
    /// The consumption of PINION-PR79 deliberately changes NOTHING observable: a `Raw` answer
    /// and the `Json` answer it replaced reach the wire as the same bytes in the same order, so
    /// every test that reads a decoded frame stays green whether the change is present or
    /// reverted. That makes the usual "assert the behaviour" discipline blind here — the claim
    /// is "the frame is serialized ONCE, not built as a tree and encoded again", and the only
    /// witness to it is the VARIANT the surface answers with.
    ///
    /// The three parametric reads are covered together because they share one reason to exist:
    /// each answers a whole structure the producer has already built, and each was a
    /// `to_value(..).map_or(Null, Json)` before.
    #[test]
    fn a_panes_structural_answers_are_encoded_text_not_a_dom() {
        let (_workspace, external) = surface();
        for path in ["cells.0", "find.a", "regex.a"] {
            let answer = external.query(path).expect("the surface owns the address");
            assert!(
                answer.as_raw().is_some(),
                "`{path}` still builds a serde_json::Value DOM for the dispatch to re-encode; \
                 the answer was {answer:?}",
            );
        }
    }

    /// The other half of the same claim: encoded-once must not mean encoded-DIFFERENTLY.
    ///
    /// A `RawJson` carries text the producer wrote, and nothing downstream re-checks it against
    /// the type it came from — so the guard that the bytes still say what the DOM path said has
    /// to be here. Parsed back, the answer is exactly `to_value` of the frame the SAME surface
    /// produces, rather than of a frame this test built a second way.
    #[test]
    fn an_encoded_answer_carries_the_document_the_dom_would_have() {
        let (_workspace, external) = surface();
        let answer = external.query("cells.0").expect("the surface answers");
        let encoded = answer.as_raw().expect("an encoded answer");
        assert_eq!(
            encoded.to_value().expect("the text is valid JSON"),
            serde_json::to_value(external.frame_at(0)).expect("the frame serialises"),
            "the spliced text and the DOM path describe the same frame",
        );
    }

    #[test]
    fn frame_bracketed_paste_wraps_multiline_text() {
        // The whole multi-line paste sits INSIDE one bracket pair, so a child holds it as one
        // paste instead of executing each line — the reason bracketed paste exists.
        assert_eq!(
            frame_bracketed_paste("git status\nls -a"),
            b"\x1b[200~git status\nls -a\x1b[201~",
        );
    }

    #[test]
    fn frame_bracketed_paste_strips_an_embedded_end_marker() {
        // Paste-injection guard: a payload carrying the END marker must NOT be able to close the
        // bracket early and have its tail (`rm -rf /`) read as typed input.
        let framed = frame_bracketed_paste("safe\x1b[201~rm -rf /");
        assert_eq!(framed, b"\x1b[200~saferm -rf /\x1b[201~".to_vec());
        // Exactly ONE end marker survives — the framing's own trailer.
        let end = b"\x1b[201~";
        let count = framed.windows(end.len()).filter(|w| *w == end).count();
        assert_eq!(count, 1, "no forged end marker survives the sanitize");
    }

    #[test]
    fn frame_bracketed_paste_strips_an_embedded_start_marker() {
        assert_eq!(
            frame_bracketed_paste("a\x1b[200~b"),
            b"\x1b[200~ab\x1b[201~".to_vec(),
        );
    }

    #[test]
    fn parses_bare_string_key() {
        let parsed = parse_key_args(&IntrospectValue::Text("a".to_string())).unwrap();
        assert_eq!(parsed, Some(("a".to_string(), Modifiers::default())));
    }

    #[test]
    fn parses_object_with_modifiers() {
        let parsed = parse_key_args(&json_args(json!({"key": "c", "ctrl": true}))).unwrap();
        assert_eq!(
            parsed,
            Some((
                "c".to_string(),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ))
        );
    }

    #[test]
    fn super_field_maps_to_sup() {
        let parsed = parse_key_args(&json_args(json!({"key": "x", "super": true}))).unwrap();
        assert_eq!(
            parsed.unwrap().1,
            Modifiers {
                sup: true,
                ..Modifiers::default()
            }
        );
    }

    #[test]
    fn key_up_edge_is_suppressed() {
        let parsed = parse_key_args(&json_args(json!({"key": "a", "state": "up"}))).unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn missing_or_empty_key_is_type_mismatch() {
        assert_eq!(
            parse_key_args(&json_args(json!({}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_key_args(&json_args(json!({"key": ""}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_key_args(&IntrospectValue::Int(1)),
            Err(InvokeError::TypeMismatch)
        );
    }

    #[test]
    fn unknown_state_is_type_mismatch() {
        assert_eq!(
            parse_key_args(&json_args(json!({"key": "a", "state": "sideways"}))),
            Err(InvokeError::TypeMismatch),
        );
    }

    #[test]
    fn parses_bare_string_text() {
        assert_eq!(
            parse_text_args(&IntrospectValue::Text("한".to_string())),
            Ok("한".to_string())
        );
        // Empty is allowed (the caller no-ops it — IME cancel-via-empty-commit).
        assert_eq!(
            parse_text_args(&IntrospectValue::Text(String::new())),
            Ok(String::new())
        );
    }

    #[test]
    fn parses_object_text() {
        assert_eq!(
            parse_text_args(&json_args(json!({"text": "안녕"}))),
            Ok("안녕".to_string())
        );
    }

    #[test]
    fn non_string_text_is_type_mismatch() {
        assert_eq!(
            parse_text_args(&json_args(json!({}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_text_args(&json_args(json!({"text": 1}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_text_args(&IntrospectValue::Int(1)),
            Err(InvokeError::TypeMismatch)
        );
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// A `sh` that puts its PTY in raw `-echo` (so the raw capture is byte-clean, not cooked /
    /// caret-mangled) and copies stdin to stdout with `cat`, optionally enabling bracketed paste
    /// (DECSET 2004) first — the child echoes whatever the host writes, so what we pasted appears
    /// verbatim in [`PanePtyHandle::raw_output`].
    fn raw_cat(enable_2004: bool) -> sprag_terminal::PanePty {
        use sprag_terminal::{CommandBuilder, PanePty};
        let script = if enable_2004 {
            "stty raw -echo 2>/dev/null; printf '\\033[?2004h'; cat"
        } else {
            "stty raw -echo 2>/dev/null; cat"
        };
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "xterm");
        PanePty::spawn(command, 40, 6).expect("spawn a pty")
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    #[test]
    fn paste_brackets_when_the_child_enabled_2004() {
        let pty = raw_cat(true);
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().bracketed_paste),
            "the child's DECSET 2004 was never emulated",
        );
        assert!(paste(&handle, "a\nb"));
        // The raw `cat` echoes the bytes the host wrote, so the bracket wrap rides the capture.
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[200~a\nb\x1b[201~")),
            "the paste reached the child UNBRACKETED (mode 2004 was on)",
        );
    }

    #[test]
    fn paste_is_raw_when_the_child_did_not_enable_2004() {
        let pty = raw_cat(false);
        let handle = pty.handle();
        assert!(!handle.input_modes().bracketed_paste);
        assert!(paste(&handle, "a\nb"));
        // The literal text echoes back with NO bracket markers (legacy, == send_text).
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"a\nb")),
            "the raw paste never reached the child",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[200~"),
            "an un-negotiated pane must NOT get bracket markers",
        );
    }

    /// A `raw -echo` `cat` that first enables mouse tracking (DECSET 1000) + SGR encoding (1006), so
    /// a report the host writes echoes back verbatim in [`PanePtyHandle::raw_output`].
    fn raw_cat_mouse() -> sprag_terminal::PanePty {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1000h\\033[?1006h'; cat");
        command.env("TERM", "xterm");
        PanePty::spawn(command, 40, 6).expect("spawn a pty")
    }

    fn press(button: MouseButton, col: u16, row: u16) -> MouseInput {
        MouseInput {
            button,
            kind: MouseEventKind::Press,
            col,
            row,
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn parse_mouse_args_reads_the_object_form() {
        let parsed = parse_mouse_args(&json_args(json!({
            "button": "right", "kind": "press", "col": 4, "row": 2, "ctrl": true,
        })))
        .expect("valid");
        assert_eq!(parsed.button, MouseButton::Right);
        assert_eq!(parsed.kind, MouseEventKind::Press);
        assert_eq!((parsed.col, parsed.row), (4, 2));
        assert!(parsed.mods.ctrl && !parsed.mods.shift);
        // An unknown button / kind, or a bare string, is a type mismatch (a machine wire).
        assert_eq!(
            parse_mouse_args(&json_args(
                json!({"button": "x", "kind": "press", "col": 0, "row": 0})
            )),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            parse_mouse_args(&IntrospectValue::Text("left".into())),
            Err(InvokeError::TypeMismatch),
        );
    }

    #[test]
    fn mouse_report_reaches_the_child_when_tracking_is_on() {
        let pty = raw_cat_mouse();
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::Click),
            "the child's DECSET 1000 was never emulated",
        );
        // A left press at cell (col 4, row 2) → SGR report ESC [ < 0 ; 5 ; 3 M (1-based).
        assert!(mouse(&handle, press(MouseButton::Left, 4, 2)));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<0;5;3M")),
            "the SGR mouse report never reached the child",
        );
    }

    #[test]
    fn a_wheel_report_reaches_the_child_when_tracking_is_on() {
        let pty = raw_cat_mouse();
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::Click),
            "the child's DECSET 1000 was never emulated",
        );
        // A wheel-up step at cell (col 4, row 2) → xterm pseudo-button 64, SGR ESC [ < 64 ; 5 ; 3 M
        // (a wheel step is a press, no release). This is the Stage 2 report the GUI's `apply_wheel`
        // sends when the pointer wheels over a tracking pane's grid.
        assert!(mouse(&handle, press(MouseButton::WheelUp, 4, 2)));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<64;5;3M")),
            "the SGR wheel report never reached the child",
        );
    }

    #[test]
    fn a_drag_report_reaches_the_child_under_button_event_tracking() {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // DECSET 1002 (button-event) + 1006 (SGR) — this level reports drag.
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1002h\\033[?1006h'; cat");
        command.env("TERM", "xterm");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");
        let handle = pty.handle();
        assert!(
            wait_until(
                || handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::ButtonEvent
            ),
            "the child's DECSET 1002 was never emulated",
        );
        // A LEFT drag at cell (col 4, row 2) -> button 0 | motion bit 32 = 32, SGR ESC[<32;5;3M
        // (the Stage 4 report the GUI sends when the pointer moves cell with a button held).
        let drag = MouseInput {
            button: MouseButton::Left,
            kind: MouseEventKind::Drag,
            col: 4,
            row: 2,
            mods: Modifiers::default(),
        };
        assert!(mouse(&handle, drag));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<32;5;3M")),
            "the SGR drag report never reached the child",
        );
    }

    #[test]
    fn focus_reports_reach_the_child_when_1004_is_on() {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1004h'; cat");
        command.env("TERM", "xterm");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().focus_tracking),
            "the child's DECSET 1004 was never emulated",
        );
        // Focus gained -> ESC [ I, focus lost -> ESC [ O (Stage 5).
        assert!(focus(&handle, true));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[I")),
            "the focus-in report never reached the child",
        );
        assert!(focus(&handle, false));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[O")),
            "the focus-out report never reached the child",
        );
    }

    #[test]
    fn a_focus_report_is_dropped_when_1004_is_off() {
        let pty = raw_cat(false); // a cat that never enables 1004
        let handle = pty.handle();
        assert!(!handle.input_modes().focus_tracking);
        // The seam accepts the edge as a no-op success — the mode wanted nothing.
        assert!(focus(&handle, true));
        // Prove nothing was written by ordering a sentinel after it.
        assert!(send_text(&handle, "SENTINEL"));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"SENTINEL")),
            "the sentinel never echoed",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[I"),
            "a pane with 1004 off must receive NO focus report",
        );
    }

    #[test]
    fn mouse_report_is_dropped_when_no_tracking_mode_is_active() {
        // A `cat` that never enables a tracking mode.
        let pty = raw_cat(false);
        let handle = pty.handle();
        assert_eq!(
            handle.input_modes().mouse_protocol,
            sprag_vt::MouseProtocol::None
        );
        // The seam accepts the event as a no-op success — the mode wanted nothing.
        assert!(mouse(&handle, press(MouseButton::Left, 4, 2)));
        // Prove NOTHING was written by ordering a sentinel after it: if a report had been written it
        // would echo before the sentinel does.
        assert!(send_text(&handle, "SENTINEL"));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"SENTINEL")),
            "the sentinel never echoed",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[<"),
            "a pane with no tracking mode must receive NO mouse report",
        );
    }

    /// ⚠ **THE FIXTURE IS WHAT THESE SHARE, NOT A HELPER THAT TAKES THE CLAIM.** A
    /// `fn(claim: impl Fn(table, Invoke<'_>) -> Driven)` reads well and does not compile across a
    /// crate boundary: the `'_` in the bound desugars to a higher-ranked lifetime the plain `fn` item
    /// cannot satisfy, and the mismatch prints two signatures that look identical. Three lines per
    /// test is the honest form, and [`surface`](self) is already the fixture.
    ///
    /// The fixture is a LIVE pane over a real PTY, so every probe goes through the parser the daemon
    /// uses and the writes it survives reach a real child.
    ///
    /// ⚠ Many of these calls come back `Rejected`, and that is the design rather than a weakness: a
    /// `cat` child enables no mouse tracking and holds no pending clipboard query, and the generic
    /// filler for a required open string is not an encodable key name. `TypeMismatch` is the
    /// discriminator — the GRAMMAR's refusal — so a call the parser read and the pane declined to
    /// perform answers the question exactly as well as one that wrote bytes.
    /// ⚠⚠ **EVERY WORD THE PANE SURFACE PUBLISHES IS A WORD IT ACCEPTS** — sixteen of them, none of
    /// which a client could discover at all before R353.
    #[test]
    fn every_published_word_is_a_word_the_pane_accepts() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::every_published_word_is_accepted(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            16,
            "one call per published word: the two key edges, the eight mouse buttons, the four \
             pointer edges, and the two clipboard selections",
        );
    }

    /// ⚠⚠ **AN ARGUMENT THIS SURFACE CONSTRAINS PUBLISHES WHAT IT ADMITS** — the gate that named
    /// `state` and `sel` when this table was first written with neither vocabulary declared.
    #[test]
    fn an_argument_the_pane_constrains_publishes_what_it_admits() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_constrained_argument_publishes_what_it_admits(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            7,
            "one probe per open string argument of every form: a key name in each of `key`'s two \
             forms, the literal text in each of `text`'s and `paste`'s two, and a clipboard answer's \
             text — every one of them a value the caller invents",
        );
    }

    /// ⚠⚠ **A DECLARED ARGUMENT IS ONE THE PANE ACTUALLY READS** — and it found four coercions the
    /// first time it ran.
    ///
    /// `ctrl`, `alt`, `shift` and `super` were read with `and_then(Value::as_bool).unwrap_or(false)`,
    /// so `{"key": "a", "ctrl": 1}` injected an UNMODIFIED `a` and answered success — while `col` and
    /// `row`, two lines away in the same file, refused a malformed value. R330's odd-one-out rule and
    /// R352b's `report_agent` defect, in the parser a keystroke goes through.
    /// ⚠⚠ **AN OPTIONAL ARGUMENT OF THE INPUT SURFACE MAY BE DECLINED AS `null`** — the third
    /// surface asked, because the defect was never about one verb: a client whose language
    /// serialises an absent optional as `null` calls EVERY verb that way.
    #[test]
    fn an_optional_argument_of_the_pane_surface_may_be_declined_as_null() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::an_optional_argument_may_be_declined_as_null(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            8,
            "one probe per OPTIONAL declared argument of every form — required ones are not \
             driven, because `null` for something the grammar demands is malformed rather than \
             declined",
        );
    }

    #[test]
    fn a_declared_argument_is_one_the_pane_reads() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_declared_argument_is_one_the_daemon_reads(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            22,
            "one probe per declared argument of every FORM: seven across `key`'s two forms, two \
             each for `text` and `paste`, seven for a mouse report, one focus edge, and three for a \
             clipboard answer",
        );
    }

    /// ⚠ **NO VERB OF THIS SURFACE TAKES NOTHING, ASSERTED RATHER THAN ASSUMED** — the tripwire that
    /// makes `a_nullary_form_is_a_verb_that_needs_nothing` start holding it the day one does.
    ///
    /// The claim exists because the GUI's five nullary verbs needed it, and R353's `FormKind` doc had
    /// said sprag had none of them. A number here is what keeps that from being a statement about the
    /// surfaces somebody happened to be looking at.
    #[test]
    fn no_verb_of_this_surface_is_nullary_yet() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            0,
            "every verb this surface serves takes arguments, so the claim drives nothing — and the \
             number is what says so",
        );
    }

    /// ⚠⚠ **A VERB THAT TAKES TWO SHAPES PUBLISHES TWO SHAPES** — built from the SERVED ANSWER, one
    /// call per published form.
    ///
    /// # The first version of this test was the defect it exists to catch
    ///
    /// It sent a bare string and an object straight to `invoke` and asserted both were read, and its
    /// own doc claimed that dropping the scalar declaration would redden it. **Measured: it stayed
    /// GREEN.** Of course it did — the daemon accepts the bare string whether or not the wire mentions
    /// it, so the test was about the PARSER while claiming to be about the PUBLICATION. R320's rule,
    /// which R352 paid once already at the ratchet level and this repeats one level down.
    ///
    /// So the shapes come off the served slot now. `text` must publish BOTH kinds, and each published
    /// form is filled and driven — a form the wire stops mentioning is a form this cannot drive, and
    /// the count says so.
    #[test]
    fn a_verb_that_takes_two_shapes_publishes_both_of_them() {
        let (_workspace, mut external) = surface();
        let published = external
            .query(crate::wire::ACTION_GRAMMAR_SLOT)
            .expect("the surface owns the address");
        let IntrospectValue::Json(Value::Object(verbs)) = published else {
            panic!("the grammar slot answers an object");
        };
        let forms = verbs[TEXT_ACTION]
            .as_array()
            .expect("text answers its forms")
            .clone();

        let mut drove: Vec<&str> = Vec::new();
        for form in &forms {
            let kind = form[crate::wire::CallForm::FORM_KEY]
                .as_str()
                .expect("a form says which shape it is");
            let args = form[crate::wire::CallForm::ARGS_KEY]
                .as_array()
                .expect("a form answers its arguments");
            // FILLED FROM THE DECLARATION ALONE, the way an agent that has read this and nothing else
            // would: a scalar form's one argument IS the value, an object form's are its members.
            let call = match kind {
                "scalar" => {
                    assert_eq!(args.len(), 1, "a scalar form carries exactly one argument");
                    sprag_conformance::as_the_wire_delivers_it(&json!("한"))
                }
                "object" => {
                    let mut map = serde_json::Map::new();
                    for arg in args {
                        map.insert(
                            arg[crate::wire::ArgGrammar::NAME_KEY]
                                .as_str()
                                .expect("an argument is named")
                                .to_owned(),
                            json!("한"),
                        );
                    }
                    json_args(Value::Object(map))
                }
                other => panic!("`{other}` is not a shape this surface publishes"),
            };
            assert!(
                external.invoke(TEXT_ACTION, call).is_ok(),
                "the {kind} form is published, so it is a call this surface reads",
            );
            drove.push(match kind {
                "scalar" => "scalar",
                _ => "object",
            });
        }
        assert_eq!(
            drove,
            ["scalar", "object"],
            "BOTH shapes are published — the bare value an IME commit sends, and the object form. A \
             verb that takes two and publishes one tells a client less than it could, and this is the \
             only gate that can see which one went missing.",
        );

        // THE CONTROL, both ways: neither form is the other's shape wearing a hat, so a client that
        // picked a published form and filled it as published cannot have picked wrong.
        assert_eq!(
            external.invoke(TEXT_ACTION, IntrospectValue::Json(json!(4242))),
            Err(InvokeError::TypeMismatch),
            "a bare NUMBER is not this verb's scalar form",
        );
        assert_eq!(
            external.invoke(TEXT_ACTION, json_args(json!({"content": "한"}))),
            Err(InvokeError::TypeMismatch),
            "and an object keyed by anything else is not its object form",
        );
    }

    /// ⚠⚠ **A VERB THIS SURFACE DOES NOT DECLARE IS A VERB IT DOES NOT RUN** — the guard this surface
    /// was the odd one out for lacking.
    ///
    /// Five surfaces ran [`declares_verb`](crate::wire::declares_verb) at their door from R352 and
    /// this one did not, so a verb added to the `match` below without a line in
    /// [`PANE_SCHEMA`](crate::wire::PANE_SCHEMA) would have been dispatched and discoverable by
    /// nobody — R352's own defect, on the surface a keybinding drives in-process most often.
    ///
    /// ⚠ **WHAT THIS TEST CAN AND CANNOT SHOW.** The guard reads a `const` schema, so no fixture can
    /// hand it a schema with a verb missing; what a test can do is show the guard is REACHED, and the
    /// MUTATION PAIR is what shows it is load-bearing. Both halves were run:
    ///
    /// * deleting `SchemaField::action(FOCUS_ACTION, "action")` from `PANE_SCHEMA` **with the guard in
    ///   place** reddens [`a_declared_argument_is_one_the_pane_reads`](self) — the verb became
    ///   unreachable, so its arguments answer `UnknownPath` where the grammar promises `TypeMismatch`,
    ///   and the gate that drives the published grammar is what reports it;
    /// * the same deletion **with the guard removed** leaves that gate GREEN: the verb is dispatched
    ///   while `$schema` never mentions it, which is R352's defect exactly and the state this surface
    ///   was in until R353.
    ///
    /// So the guard is what makes an undeclared arm unreachable, and the count below is a second,
    /// independent instrument — it noticed the undeclaration in BOTH halves, because it reads the
    /// schema rather than the dispatch.
    #[test]
    fn a_verb_this_surface_does_not_declare_is_a_verb_it_does_not_run() {
        let (_workspace, mut external) = surface();

        // Every declared verb is reachable — the half that makes the guard's cost visible rather
        // than theoretical, and derived from the SCHEMA so a verb added there joins this loop.
        let declared: Vec<&str> = crate::wire::PANE_SCHEMA
            .iter()
            .filter(|field| field.channel == pinion_core::external::SchemaChannel::Invoke)
            .map(|field| field.path)
            .collect();
        assert_eq!(declared.len(), 6, "this surface declares six verbs");
        for verb in declared {
            assert_ne!(
                external.invoke(verb, IntrospectValue::Null),
                Err(InvokeError::UnknownPath),
                "`{verb}` is declared, so the guard lets it through to its own parser",
            );
        }

        // And a name this surface does not declare is refused as UNKNOWN rather than reaching a
        // parser — the same answer a daemon too old to know it would give.
        assert_eq!(
            external.invoke("type_this_for_me", IntrospectValue::Null),
            Err(InvokeError::UnknownPath),
        );
    }

    /// The pane surface ANSWERS its own grammar, and the answer describes the verbs IT serves.
    ///
    /// ⚠ Read off the surface, not off the table: a slot that stopped answering — or one answering
    /// the multiplexer's table by a copy-paste — fails here, which is R320's rule applied to the
    /// second surface that publishes one.
    #[test]
    fn the_pane_surface_answers_how_to_call_its_own_verbs() {
        let (_workspace, external) = surface();
        let answer = external
            .query(crate::wire::ACTION_GRAMMAR_SLOT)
            .expect("the surface owns the address");
        let IntrospectValue::Json(Value::Object(verbs)) = answer else {
            panic!("the grammar slot answers an object");
        };

        let mut published: Vec<&String> = verbs.keys().collect();
        published.sort_unstable();
        assert_eq!(
            published,
            [
                CLIPBOARD_ANSWER_ACTION,
                FOCUS_ACTION,
                KEY_ACTION,
                MOUSE_ACTION,
                PASTE_ACTION,
                TEXT_ACTION
            ],
            "the six verbs this surface serves, and not the multiplexer's",
        );

        // `key`'s two forms, with the shapes that make them two — the fact no array of arguments
        // could have carried.
        let forms = verbs[KEY_ACTION].as_array().expect("key answers its forms");
        let shapes: Vec<&str> = forms
            .iter()
            .map(|form| {
                form[crate::wire::CallForm::FORM_KEY]
                    .as_str()
                    .unwrap_or("?")
            })
            .collect();
        assert_eq!(shapes, ["scalar", "object"]);
        assert_eq!(
            forms[0][crate::wire::CallForm::ARGS_KEY]
                .as_array()
                .expect("a form answers its arguments")
                .len(),
            1,
            "a scalar form carries exactly one argument: the whole value",
        );
    }
}
