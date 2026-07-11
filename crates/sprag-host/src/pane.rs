//! The terminal pane as a pinion `External` — the engine side of the
//! R1.7 split.
//!
//! PINION-REQUIREMENTS R1.7 separates the pane into *data* and *engine*:
//! the cell grid is exposed as a `Scene::TextGrid` (introspectable
//! projection), while the PTY+emulator engine sits behind an `External`
//! boundary (process-side opacity justified). [`SpragPaneExternal`] is that
//! engine surface. It carries no scene state of its own — only a
//! [`SessionHandle`] onto the live producer — so input is a *producer*
//! mutation reached through pinion's canonical `scene/invoke`, not a
//! mutation of pinion's projection (R969: pinion projects, the producer
//! owns state).
//!
//! The action channel is the R2.6 input seam: `invoke("key", {key, …})`
//! encodes the W3C key + modifiers to PTY bytes ([`sprag_input::encode`],
//! sprag-owned) and writes them to the child. A sibling `invoke("text",
//! {text})` writes **literal** UTF-8 to the child (no key-encoding) — the seam
//! for IME-composed input (a Hangul/CJK commit is text, not a keystroke) and
//! for pasting; the AI peer drives the same wire. A read-shaped `invoke("cells",
//! {offset})` returns the pane's cell FRAME — the projected [`GridBuffer`](pinion_core::GridBuffer) at that
//! scrollback offset (serde-able since PINION-PR49) plus the scroll facts
//! (scrollback depth + visible rows) that ride with it — the wire display client's
//! per-frame read (topology B: the client reconstructs the exact buffer the host
//! projected and paints it, so "read data, not pixels" reaches the human path). It
//! is an `invoke` rather than a `query` because it carries the `offset` parameter,
//! which the path-only `scene/query` cannot. The read channel exposes the
//! producer-owned input modes (`query("application_cursor_keys")`) and the
//! pane's full output text (`query("full_text")`, scrollback + visible) — the
//! same `Screen::full_text` the in-process capture path reads, so an external
//! peer and a plugin share one notion of the screen.

use std::fmt;

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
};
use serde_json::{Value, json};
use sprag_input::Modifiers;
use sprag_terminal::SessionHandle;
use sprag_vt::Screen;

use crate::external::rpc_external_impl;

/// The invoke action that injects a key into the focused pane.
const KEY_ACTION: &str = "key";
/// The invoke action that writes literal UTF-8 text into the pane (no
/// key-encoding) — IME commit / paste. See [`SpragPaneExternal::inject_text`].
const TEXT_ACTION: &str = "text";
/// The invoke action returning one pane's cell FRAME — the wire display client's
/// per-frame read. See [`SpragPaneExternal::read_cells`].
const CELLS_ACTION: &str = "cells";
/// The query slot reporting the producer's DECCKM (application cursor
/// keys) state.
const CURSOR_KEYS_SLOT: &str = "application_cursor_keys";
/// The query slot reporting the pane's full output text (scrollback +
/// visible) — the same [`Screen::full_text`] the in-process capture path
/// reads, so an external peer and a plugin see one notion of the screen.
const FULL_TEXT_SLOT: &str = "full_text";

/// Encode a W3C `key` + `mods` to PTY bytes (the sprag-owned R2.6 encoder,
/// [`sprag_input::encode`]) and write them to `session`. `true` on success;
/// `false` if the key is unencodable or the write failed.
///
/// This is the key->PTY SSOT shared by the RPC input surface
/// ([`SpragPaneExternal`]'s `key` action, which parses the JSON/scene wire) and an
/// in-process display client (`sprag-gui`'s `LocalHost`, which calls this directly
/// with typed args) — so the human keyboard path and the AI `scene/invoke` path
/// encode IDENTICALLY.
#[must_use]
pub fn send_key(session: &SessionHandle, key: &str, mods: Modifiers) -> bool {
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
pub fn send_text(session: &SessionHandle, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    session.write(text.as_bytes()).is_ok()
}

/// The pane engine `External`: a thin, scene-stateless forwarder onto the
/// live [`SessionHandle`]. Input arrives via `scene/invoke` and is encoded
/// to PTY bytes by the sprag-owned encoder (R2.6); the producer's reader
/// thread lives behind this boundary, so the engine is `UiThreadSync` from
/// pinion's vantage (it does its work synchronously when invoked).
pub struct SpragPaneExternal {
    session: SessionHandle,
}

impl SpragPaneExternal {
    /// Build the engine surface over a live session's I/O handle.
    #[must_use]
    pub fn new(session: SessionHandle) -> Self {
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

    /// Return the pane's cell FRAME at scrollback `offset` — the wire display
    /// client's per-frame read (topology B). The frame is a JSON object:
    ///
    /// * `cells` — the projected [`GridBuffer`](pinion_core::GridBuffer)
    ///   ([`sprag_grid::project_scrolled`], serde-able since PINION-PR49), the
    ///   paint-authoritative buffer the client reconstructs byte-for-byte;
    /// * `scrollback_len` — the retained history depth (the scrollbar extent + the
    ///   top-anchored offset math);
    /// * `visible_rows` — one scrollback page.
    ///
    /// The three are read under ONE screen lock — an atomically consistent snapshot
    /// (the cells and the scroll facts describe the SAME screen state, never a torn
    /// read across two locks) — and the [`GridBuffer`](pinion_core::GridBuffer) is
    /// serialized AFTER the lock is released, so the (CPU-bound) serialization never
    /// holds the producer's screen. `offset == 0` is the live view; a larger offset
    /// windows into history ([`sprag_grid::project_scrolled`] self-clamps to the
    /// retained depth). A malformed `offset` is an [`InvokeError::TypeMismatch`]; a
    /// serialization failure (never expected for a valid buffer) is
    /// [`InvokeError::Rejected`].
    fn read_cells(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let offset = parse_offset_arg(args)?;
        // One screen lock: project the scrolled cells + read the scroll facts that
        // ride with them, so the frame is a consistent snapshot. Serialization runs
        // after, off the lock.
        let (cells, scrollback_len, visible_rows) = self.session.with_screen(|screen| {
            (
                sprag_grid::project_scrolled(screen, offset),
                screen.scrollback_len(),
                screen.rows(),
            )
        });
        let cells = serde_json::to_value(&cells).map_err(|_| InvokeError::Rejected)?;
        Ok(IntrospectValue::Json(json!({
            "cells": cells,
            "scrollback_len": scrollback_len,
            "visible_rows": visible_rows,
        })))
    }
}

impl fmt::Debug for SpragPaneExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `SessionHandle` wraps un-`Debug` PTY/emulator handles; the engine
        // is identified structurally (External: Debug is required by §5.2).
        f.debug_struct("SpragPaneExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(SpragPaneExternal);

impl ExternalIntrospect for SpragPaneExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(&[
            (KEY_ACTION, "action"),
            (TEXT_ACTION, "action"),
            (CELLS_ACTION, "action"),
            (CURSOR_KEYS_SLOT, "bool"),
            (FULL_TEXT_SLOT, "string"),
        ])
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            CURSOR_KEYS_SLOT => Some(IntrospectValue::Bool(
                self.session.input_modes().application_cursor_keys,
            )),
            FULL_TEXT_SLOT => Some(IntrospectValue::Text(
                self.session.with_screen(Screen::full_text),
            )),
            _ => None,
        }
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots: input is an action (invoke `key`) and the
        // cursor-keys mode is producer-owned (read-only here).
        Err(InterveneError::UnknownPath)
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            KEY_ACTION => self.inject_key(&args),
            TEXT_ACTION => self.inject_text(&args),
            CELLS_ACTION => self.read_cells(&args),
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

/// Parse the `cells` action's args into the scrollback `offset` (rows up from the
/// live bottom). Accepts `null` (→ `0`, the live view), a bare non-negative integer,
/// or an object `{offset: N}` (absent `offset` → `0`). A negative or non-integer
/// `offset` — a client bug — is an [`InvokeError::TypeMismatch`]. Over-large offsets
/// are NOT rejected here: [`sprag_grid::project_scrolled`] self-clamps to the
/// retained scrollback depth, so a client that asks past the top gets the top.
fn parse_offset_arg(args: &IntrospectValue) -> Result<usize, InvokeError> {
    let from_json = |v: &Value| -> Result<usize, InvokeError> {
        v.as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)
    };
    match args {
        IntrospectValue::Null => Ok(0),
        IntrospectValue::Int(n) => usize::try_from(*n).map_err(|_| InvokeError::TypeMismatch),
        IntrospectValue::Json(Value::Object(map)) => match map.get("offset") {
            None => Ok(0),
            Some(v) => from_json(v),
        },
        IntrospectValue::Json(v) => from_json(v),
        _ => Err(InvokeError::TypeMismatch),
    }
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
    fn parse_offset_defaults_to_live_and_rejects_negatives() {
        // Null / absent offset = the live view.
        assert_eq!(parse_offset_arg(&IntrospectValue::Null), Ok(0));
        assert_eq!(parse_offset_arg(&json_args(json!({}))), Ok(0));
        // A non-negative offset, from an object or a bare int.
        assert_eq!(parse_offset_arg(&json_args(json!({"offset": 7}))), Ok(7));
        assert_eq!(parse_offset_arg(&IntrospectValue::Int(3)), Ok(3));
        assert_eq!(parse_offset_arg(&json_args(json!(5))), Ok(5));
        // A negative or non-integer offset is a client bug.
        assert_eq!(
            parse_offset_arg(&IntrospectValue::Int(-1)),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_offset_arg(&json_args(json!({"offset": -2}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_offset_arg(&IntrospectValue::Text("x".to_string())),
            Err(InvokeError::TypeMismatch)
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
