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
//! for IME-composed input (a Hangul/CJK commit is text, not a keystroke) and
//! for pasting; the AI peer drives the same wire.
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
use serde_json::Value;
use sprag_input::Modifiers;
use sprag_terminal::PanePtyHandle;
use sprag_vt::Screen;

use crate::external::rpc_external_impl;
use crate::host::PaneScrollFacts;

// The action names + query slots this external answers are the shared wire ABI
// vocabulary ([`crate::wire`]) — the SAME consts the wire client addresses, so the
// two cannot drift.
use crate::wire::{
    CELLS_FIELD, CURSOR_KEYS_SLOT, FRAMES_SLOT, FULL_TEXT_SLOT, KEY_ACTION, PANE_SCHEMA,
    TEXT_ACTION,
};

/// Encode a W3C `key` + `mods` to PTY bytes (the sprag-owned R2.6 encoder,
/// [`sprag_input::encode`]) and write them to `session`. `true` on success;
/// `false` if the key is unencodable or the write failed.
///
/// This is the key->PTY SSOT shared by the RPC input surface
/// ([`SpragPaneExternal`]'s `key` action, which parses the JSON/scene wire) and the
/// in-process display client ([`HostClient::send_key`](crate::HostClient::send_key), which calls
/// this directly with typed args) — so the human keyboard path and the AI
/// `scene/invoke` path encode IDENTICALLY.
#[must_use]
pub fn send_key(session: &PanePtyHandle, key: &str, mods: Modifiers) -> bool {
    match sprag_input::encode(key, mods, session.input_modes()) {
        Some(bytes) => session.write(&bytes).is_ok(),
        None => false,
    }
}

/// Write literal UTF-8 `text` to `session` (no key-encoding) — the IME-commit /
/// paste seam. Empty text is a no-op success. `true` on success; `false` on a
/// write failure. The text->PTY SSOT shared by [`SpragPaneExternal`]'s `text`
/// action and the in-process client.
#[must_use]
pub fn send_text(session: &PanePtyHandle, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    session.write(text.as_bytes()).is_ok()
}

/// The pane engine `External`: a thin, scene-stateless forwarder onto the
/// live [`PanePtyHandle`]. Input arrives via `scene/invoke` and is encoded
/// to PTY bytes by the sprag-owned encoder (R2.6); the producer's reader
/// thread lives behind this boundary, so the engine is `UiThreadSync` from
/// pinion's vantage (it does its work synchronously when invoked).
pub struct SpragPaneExternal {
    session: PanePtyHandle,
}

impl SpragPaneExternal {
    /// Build the engine surface over a live session's I/O handle.
    #[must_use]
    pub fn new(session: PanePtyHandle) -> Self {
        Self { session }
    }

    /// Encode a `key` action's args and write the resulting bytes to the
    /// PTY. A `state:"up"` edge is a no-op success (terminals emit no
    /// release in this mode). An unencodable key or a write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_key(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let Some((key, mods)) = parse_key_args(args)? else {
            return Ok(IntrospectValue::Null); // suppressed key-up edge
        };
        if send_key(&self.session, &key, mods) {
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
        if send_text(&self.session, &text) {
            Ok(IntrospectValue::Null)
        } else {
            Err(InvokeError::Rejected)
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
        self.session.with_screen(|screen| CellFrame {
            cells: sprag_grid::project_scrolled(screen, offset),
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
    /// The projected paint-authoritative cell buffer (serde-able since PINION-PR49).
    pub cells: GridBuffer,
    /// The non-cell per-frame facts, flattened so `scrollback_len` / `visible_rows`
    /// are top-level wire keys (their names come from [`PaneScrollFacts`], the SSOT).
    #[serde(flatten)]
    pub facts: PaneScrollFacts,
}

impl fmt::Debug for SpragPaneExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `PanePtyHandle` wraps un-`Debug` PTY/emulator handles; the engine
        // is identified structurally (External: Debug is required by §5.2).
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
            return Some(cells_offset(arg).map_or(IntrospectValue::Null, |offset| {
                serde_json::to_value(self.frame_at(offset))
                    .map_or(IntrospectValue::Null, IntrospectValue::Json)
            }));
        }
        match path {
            // The count that bounds `cells.<offset>` (`IndexOf(FRAMES_SLOT)`): the live view
            // plus one per retained history line. An agent reads this scalar to learn where
            // history ends, instead of fetching whole cell grids to find out.
            FRAMES_SLOT => Some(IntrospectValue::Int(
                i64::try_from(self.session.with_screen(Screen::scrollback_len)).unwrap_or(i64::MAX)
                    + 1,
            )),
            CURSOR_KEYS_SLOT => Some(IntrospectValue::Bool(
                self.session.input_modes().application_cursor_keys,
            )),
            FULL_TEXT_SLOT => Some(IntrospectValue::Text(
                self.session.with_screen(Screen::full_text),
            )),
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
            TEXT_ACTION => self.inject_text(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_args(v: serde_json::Value) -> IntrospectValue {
        IntrospectValue::Json(v)
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
}
