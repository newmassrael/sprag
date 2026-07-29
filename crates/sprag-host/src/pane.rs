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
use sprag_input::{Modifiers, MouseButton, MouseEventKind, MouseInput};
use sprag_terminal::PanePtyHandle;
use sprag_vt::{ClipboardTarget, Screen, osc52_reply};

use crate::external::rpc_external_impl;
use crate::host::PaneScrollFacts;

// The action names + query slots this external answers are the shared wire ABI
// vocabulary ([`crate::wire`]) — the SAME consts the wire client addresses, so the
// two cannot drift.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::wire::{
    CELLS_FIELD, CLIPBOARD_ANSWER_ACTION, CLIPBOARD_WRITE_SLOT, CURSOR_KEYS_SLOT, FIND_FIELD,
    FOCUS_ACTION, FRAMES_SLOT, FULL_TEXT_SLOT, IMAGE_DATA_FIELD, KEY_ACTION, LAST_COMMAND_SLOT,
    LINKS_SLOT, MOUSE_ACTION, PANE_SCHEMA, PASTE_ACTION, PROMPT_MARKS_SLOT, REGEX_FIELD,
    TEXT_ACTION,
};

/// Encode a W3C `key` + `mods` to PTY bytes (the sprag-owned R2.6 encoder,
/// [`sprag_input::encode`]) and write them to `pty`. `true` on success;
/// `false` if the key is unencodable or the write failed.
///
/// This is the key->PTY SSOT shared by the RPC input surface
/// ([`SpragPaneExternal`]'s `key` action, which parses the JSON/scene wire) and the
/// in-process display client ([`HostClient::send_key`](crate::HostClient::send_key), which calls
/// this directly with typed args) — so the human keyboard path and the AI
/// `scene/invoke` path encode IDENTICALLY.
#[must_use]
pub fn send_key(pty: &PanePtyHandle, key: &str, mods: Modifiers) -> bool {
    match sprag_input::encode(key, mods, pty.input_modes()) {
        Some(bytes) => pty.write(&bytes).is_ok(),
        None => false,
    }
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
        if send_key(&self.pty, &key, mods) {
            Ok(IntrospectValue::Null)
        } else {
            Err(InvokeError::Rejected)
        }
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
            Err(InvokeError::Rejected)
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
            Err(InvokeError::Rejected)
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
            Err(InvokeError::Rejected)
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
            Err(InvokeError::Rejected)
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
            Err(_) => Err(InvokeError::Rejected),
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
            facts: PaneScrollFacts::from_screen(screen),
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
            let found = crate::PaneFind::from_screen_result(
                &self.pty.with_screen(|screen| screen.find(needle)),
            );
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
            let found = match self.pty.with_screen(|screen| screen.find_regex(pattern)) {
                Ok(found) => crate::PaneFind::from_screen_result(&found),
                Err(bad) => crate::PaneFind::refused(&bad),
            };
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

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
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
    let target = match map.get("sel").and_then(Value::as_str) {
        Some("c") => ClipboardTarget::Clipboard,
        Some("p") => ClipboardTarget::Primary,
        _ => return Err(InvokeError::TypeMismatch),
    };
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
            match map.get("state").and_then(Value::as_str) {
                Some("up") => return Ok(None),
                Some("down") | None => {}
                Some(_) => return Err(InvokeError::TypeMismatch),
            }
            let flag = |name: &str| map.get(name).and_then(Value::as_bool).unwrap_or(false);
            let mods = Modifiers {
                ctrl: flag("ctrl"),
                alt: flag("alt"),
                shift: flag("shift"),
                sup: flag("super"),
            };
            Ok(Some((key.to_string(), mods)))
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
    let button = match map.get("button").and_then(Value::as_str) {
        Some("left") => MouseButton::Left,
        Some("middle") => MouseButton::Middle,
        Some("right") => MouseButton::Right,
        Some("wheelup") => MouseButton::WheelUp,
        Some("wheeldown") => MouseButton::WheelDown,
        Some("wheelleft") => MouseButton::WheelLeft,
        Some("wheelright") => MouseButton::WheelRight,
        Some("none") => MouseButton::None,
        _ => return Err(InvokeError::TypeMismatch),
    };
    let kind = match map.get("kind").and_then(Value::as_str) {
        Some("press") => MouseEventKind::Press,
        Some("release") => MouseEventKind::Release,
        Some("drag") => MouseEventKind::Drag,
        Some("motion") => MouseEventKind::Motion,
        _ => return Err(InvokeError::TypeMismatch),
    };
    let coord = |name: &str| -> Result<u16, InvokeError> {
        map.get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)
    };
    let flag = |name: &str| map.get(name).and_then(Value::as_bool).unwrap_or(false);
    Ok(MouseInput {
        button,
        kind,
        col: coord("col")?,
        row: coord("row")?,
        mods: Modifiers {
            ctrl: flag("ctrl"),
            alt: flag("alt"),
            shift: flag("shift"),
            sup: false,
        },
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
}
